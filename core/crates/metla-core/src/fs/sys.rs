//! Thin wrappers that turn C return conventions into `Result`.

use std::ffi::{CStr, CString, OsStr};
use std::num::NonZeroU64;
use std::os::unix::ffi::OsStrExt;

use crate::domain::{EntryKind, EntryMeta, UnixTime};

/// `SF_DATALESS` from `<sys/stat.h>`: the file's data is not on this disk.
const SF_DATALESS: u32 = 0x4000_0000;

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

pub fn meta_of(st: &libc::stat) -> EntryMeta {
    EntryMeta {
        kind: kind_of(st.st_mode),
        blocks: st.st_blocks.unsigned_abs(),
        links: NonZeroU64::new(u64::from(st.st_nlink)).unwrap_or(NonZeroU64::MIN),
        device: device_id(st.st_dev),
        inode: st.st_ino,
        mtime: UnixTime(st.st_mtime),
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
