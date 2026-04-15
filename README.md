# rustymount

Mount ZIP/JAR archives as read-only FUSE filesystems (macOS and Linux).

## Install

```sh
curl -fsSL https://github.com/mauhiz/rustymount/releases/latest/download/install.sh | sh
```

Or with a specific version / install directory:

```sh
TAG=v0.1.0 INSTALL_DIR=~/.local/bin sh <(curl -fsSL https://raw.githubusercontent.com/mauhiz/rustymount/main/install.sh)
```

**Prerequisites**

- macOS: `brew install --cask macfuse`
- Linux: `sudo apt-get install libfuse3-dev` (or the equivalent for your distro)

## Build from source

```sh
cargo build --release
```

## Usage:

```sh
mkdir /tmp/mnt
rustymount some.jar /tmp/mnt
# browse contents, then:
umount /tmp/mnt
```

## Key design notes

- **Inode tree** is built at startup from the ZIP central directory — flat paths like `com/example/Foo.class` produce virtual intermediate directory inodes.
- **Decompression is lazy + cached** per inode: deflate entries can't be random-accessed, so the first `read` decompresses the whole entry into memory; subsequent reads slice from the cache.
- **Read-only**, `AutoUnmount` on process exit.