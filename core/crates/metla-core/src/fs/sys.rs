//! Thin wrappers that turn C return conventions into `Result`.

use std::ffi::{CStr, CString, OsStr};
use std::num::NonZeroU64;
use std::os::unix::ffi::OsStrExt;

use crate::domain::{EntryKind, EntryMeta, Stamp, UnixTime};

/// `SF_DATALESS` from `<sys/stat.h>`: the file's data is not on this disk.
pub const SF_DATALESS: u32 = 0x4000_0000;

/// Nanoseconds in a second.
pub const NANOS: i64 = 1_000_000_000;

/// The calling thread's `errno`.
pub fn errno() -> i32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

/// A C call's result: `-1` means failure with `errno` set.
pub fn check(ret: libc::c_int) -> Result<libc::c_int, i32> {
    if ret == -1 { Err(errno()) } else { Ok(ret) }
}

/// Repeats `call` while it fails with `EINTR`: a signal that arrived during
/// a system call is not a reason to give up on a directory.
pub fn retry_interrupted<T>(mut call: impl FnMut() -> Result<T, i32>) -> Result<T, i32> {
    loop {
        match call() {
            Err(libc::EINTR) => continue,
            other => return other,
        }
    }
}

/// [`check`] of a call, repeated while it is interrupted.
pub fn checked(mut call: impl FnMut() -> libc::c_int) -> Result<libc::c_int, i32> {
    retry_interrupted(|| check(call()))
}

/// A name as a C string. Names from the file system never contain NUL;
/// one that does cannot exist, which is reported as `ENOENT`.
pub fn c_name(name: &OsStr) -> Result<CString, i32> {
    CString::new(name.as_bytes()).map_err(|_| libc::ENOENT)
}

/// The bytes of a NUL-terminated C array, such as a mount point name.
pub fn c_array_bytes(array: &[libc::c_char]) -> &[u8] {
    // SAFETY: the kernel NUL-terminates these fixed-size name arrays, so the
    // string ends inside `array`.
    unsafe { CStr::from_ptr(array.as_ptr()) }.to_bytes()
}

/// An entry's metadata from `stat`. A directory's own link count is not
/// used (it contributes nothing) and is reported as one, as
/// `getattrlistbulk` reports it.
pub fn meta_of(st: &libc::stat) -> EntryMeta {
    let kind = kind_of(st.st_mode);
    let links = match kind {
        EntryKind::Dir => NonZeroU64::MIN,
        _ => NonZeroU64::new(u64::from(st.st_nlink)).unwrap_or(NonZeroU64::MIN),
    };
    EntryMeta {
        kind,
        blocks: st.st_blocks.unsigned_abs(),
        links,
        device: device_id(st.st_dev),
        inode: st.st_ino,
        mtime: UnixTime(st.st_mtime),
        changed: Stamp(st.st_ctime * NANOS + st.st_ctime_nsec),
        dataless: st.st_flags & SF_DATALESS != 0,
    }
}

/// `dev_t` is a signed 32-bit number on macOS; keep its bit pattern.
pub fn device_id(dev: libc::dev_t) -> u64 {
    u64::from(dev.cast_unsigned())
}

fn kind_of(mode: libc::mode_t) -> EntryKind {
    match mode & libc::S_IFMT {
        libc::S_IFDIR => EntryKind::Dir,
        libc::S_IFREG => EntryKind::File,
        libc::S_IFLNK => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}
