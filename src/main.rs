use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Parser;
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    Request,
};

const TTL: Duration = Duration::from_secs(1);

struct Entry {
    inode: u64,
    name: String,
    kind: FileType,
    zip_index: Option<usize>,
    size: u64,
    mtime: SystemTime,
    children: Vec<u64>,
    parent: u64,
}

struct ZipFs {
    archive: Mutex<zip::ZipArchive<std::fs::File>>,
    inodes: HashMap<u64, Entry>,
    cache: Mutex<HashMap<u64, Vec<u8>>>,
    uid: u32,
    gid: u32,
}

fn zip_datetime_to_system_time(dt: zip::DateTime) -> SystemTime {
    time::OffsetDateTime::try_from(dt)
        .map(SystemTime::from)
        .unwrap_or(UNIX_EPOCH)
}

impl ZipFs {
    fn new(path: &Path) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;

        let mut inodes: HashMap<u64, Entry> = HashMap::new();
        let mut next_inode: u64 = 2;
        let mut path_to_inode: HashMap<String, u64> = HashMap::new();

        // Inode 1 is always the root directory
        inodes.insert(
            1,
            Entry {
                inode: 1,
                name: String::new(),
                kind: FileType::Directory,
                zip_index: None,
                size: 0,
                mtime: UNIX_EPOCH,
                children: Vec::new(),
                parent: 1,
            },
        );
        path_to_inode.insert(String::new(), 1);

        // Collect entry metadata upfront to avoid borrow issues
        let mut zip_entries: Vec<(usize, String, u64, bool, SystemTime)> = Vec::new();
        for i in 0..archive.len() {
            if let Ok(e) = archive.by_index(i) {
                let mtime = e
                    .last_modified()
                    .map(zip_datetime_to_system_time)
                    .unwrap_or(UNIX_EPOCH);
                zip_entries.push((i, e.name().to_owned(), e.size(), e.is_dir(), mtime));
            }
        }

        for (zip_idx, full_name, size, is_dir, mtime) in zip_entries {
            let clean = full_name.trim_end_matches('/');
            if clean.is_empty() {
                continue;
            }

            let parts: Vec<&str> = clean.split('/').collect();

            // Walk path components, creating intermediate directories as needed
            for depth in 0..parts.len() {
                let path_key = parts[..=depth].join("/");
                if path_to_inode.contains_key(&path_key) {
                    continue;
                }

                let is_last = depth == parts.len() - 1;
                let inode = next_inode;
                next_inode += 1;

                let parent_key = if depth == 0 {
                    String::new()
                } else {
                    parts[..depth].join("/")
                };
                let &parent_inode = path_to_inode.get(&parent_key).unwrap();

                let (kind, zip_index, entry_size, entry_mtime) = if is_last {
                    if is_dir {
                        (FileType::Directory, None, 0, mtime)
                    } else {
                        (FileType::RegularFile, Some(zip_idx), size, mtime)
                    }
                } else {
                    (FileType::Directory, None, 0, UNIX_EPOCH)
                };

                inodes.insert(
                    inode,
                    Entry {
                        inode,
                        name: parts[depth].to_string(),
                        kind,
                        zip_index,
                        size: entry_size,
                        mtime: entry_mtime,
                        children: Vec::new(),
                        parent: parent_inode,
                    },
                );
                path_to_inode.insert(path_key, inode);
                inodes.get_mut(&parent_inode).unwrap().children.push(inode);
            }
        }

        Ok(Self {
            archive: Mutex::new(archive),
            inodes,
            cache: Mutex::new(HashMap::new()),
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
        })
    }

    fn file_attr(&self, entry: &Entry) -> FileAttr {
        FileAttr {
            ino: entry.inode,
            size: entry.size,
            blocks: (entry.size + 511) / 512,
            atime: entry.mtime,
            mtime: entry.mtime,
            ctime: entry.mtime,
            crtime: entry.mtime,
            kind: entry.kind,
            perm: if entry.kind == FileType::Directory { 0o755 } else { 0o644 },
            nlink: if entry.kind == FileType::Directory { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            flags: 0,
            blksize: 512,
        }
    }

    fn decompress(&self, inode: u64, zip_index: usize) -> anyhow::Result<Vec<u8>> {
        {
            let cache = self.cache.lock().unwrap();
            if let Some(data) = cache.get(&inode) {
                return Ok(data.clone());
            }
        }
        let buf = {
            let mut archive = self.archive.lock().unwrap();
            let mut entry = archive.by_index(zip_index)?;
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            buf
            // entry and archive (MutexGuard) dropped here
        };
        self.cache.lock().unwrap().insert(inode, buf.clone());
        Ok(buf)
    }
}

impl Filesystem for ZipFs {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = name.to_string_lossy();
        // Clone children to release borrow on self.inodes before looking up each child
        let children: Vec<u64> = match self.inodes.get(&parent) {
            Some(e) => e.children.clone(),
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        match children
            .iter()
            .find_map(|&ino| self.inodes.get(&ino).filter(|e| e.name == name_str.as_ref()))
        {
            Some(e) => reply.entry(&TTL, &self.file_attr(e), 0),
            None => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        match self.inodes.get(&ino) {
            Some(e) => reply.attr(&TTL, &self.file_attr(e)),
            None => reply.error(libc::ENOENT),
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let zip_index = match self.inodes.get(&ino) {
            Some(e) => match e.zip_index {
                Some(idx) => idx,
                None => {
                    reply.error(libc::EISDIR);
                    return;
                }
            },
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        match self.decompress(ino, zip_index) {
            Ok(data) => {
                let start = offset as usize;
                let end = (start + size as usize).min(data.len());
                reply.data(if start < data.len() { &data[start..end] } else { &[] });
            }
            Err(e) => {
                eprintln!("read error: {e}");
                reply.error(libc::EIO);
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let (parent, children) = match self.inodes.get(&ino) {
            Some(e) if e.kind == FileType::Directory => (e.parent, e.children.clone()),
            Some(_) => {
                reply.error(libc::ENOTDIR);
                return;
            }
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let mut all = vec![
            (ino, FileType::Directory, ".".to_string()),
            (parent, FileType::Directory, "..".to_string()),
        ];
        for child_ino in children {
            if let Some(e) = self.inodes.get(&child_ino) {
                all.push((e.inode, e.kind, e.name.clone()));
            }
        }

        for (i, (inode, kind, name)) in all.iter().enumerate().skip(offset as usize) {
            if reply.add(*inode, (i + 1) as i64, *kind, name) {
                break;
            }
        }
        reply.ok();
    }
}

#[derive(Parser)]
#[command(
    name = "rustymount",
    about = "Mount ZIP/JAR files as read-only FUSE filesystems",
    after_help = "Prerequisites: macFUSE on macOS (brew install --cask macfuse)\nUnmount with: umount <mountpoint>  (or diskutil unmount on macOS)"
)]
struct Cli {
    /// ZIP or JAR file to mount
    archive: std::path::PathBuf,
    /// Directory to mount on (must exist and be empty)
    mountpoint: std::path::PathBuf,
    /// Allow access by other users (requires user_allow_other in /etc/fuse.conf)
    #[arg(long)]
    allow_other: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let fs = ZipFs::new(&cli.archive)?;

    let mut options = vec![
        MountOption::RO,
        MountOption::FSName(cli.archive.display().to_string()),
        MountOption::AutoUnmount,
    ];
    if cli.allow_other {
        options.push(MountOption::AllowOther);
    }

    eprintln!(
        "Mounting {} → {}",
        cli.archive.display(),
        cli.mountpoint.display()
    );
    eprintln!(
        "Unmount with: umount {}",
        cli.mountpoint.display()
    );

    fuser::mount2(fs, &cli.mountpoint, &options)?;
    Ok(())
}
