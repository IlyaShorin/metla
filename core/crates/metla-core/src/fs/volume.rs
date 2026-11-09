use std::ffi::OsStr;
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::sys::{c_array_bytes, c_name, check};
use crate::domain::Bytes;

/// The file system a path lives on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub total: Bytes,
    /// Free space available to an unprivileged user.
    pub available: Bytes,
    pub mount_point: PathBuf,
    /// The id files on this volume report as their device.
    pub device: u64,
}

/// A mounted file system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub path: PathBuf,
    pub device: u64,
}

pub fn volume(path: &Path) -> io::Result<Volume> {
    let path = c_name(path.as_os_str()).map_err(io::Error::from_raw_os_error)?;
    let mut st = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `st` is writable statfs storage.
    check(unsafe { libc::statfs(path.as_ptr(), st.as_mut_ptr()) })
        .map_err(io::Error::from_raw_os_error)?;
    // SAFETY: statfs succeeded, so it initialised `st`.
    let st = unsafe { st.assume_init_ref() };
    let block = u64::from(st.f_bsize);
    Ok(Volume {
        total: Bytes(st.f_blocks * block),
        available: Bytes(st.f_bavail * block),
        mount_point: mount_point(st),
        device: fs_device(st),
    })
}

/// Every mounted file system, without waiting for unresponsive ones.
pub fn mounts() -> io::Result<Vec<Mount>> {
    let mut room = 1;
    loop {
        let table = mount_table(room)?;
        if table.len() < room {
            return Ok(table
                .iter()
                .map(|st| Mount {
                    path: mount_point(st),
                    device: fs_device(st),
                })
                .collect());
        }
        // A full table may have been cut short: ask again with more room.
        room *= 2;
    }
}

/// Up to `room` records of the mount table.
fn mount_table(room: usize) -> io::Result<Vec<libc::statfs>> {
    let mut table: Vec<libc::statfs> = Vec::with_capacity(room);
    let bytes = libc::c_int::try_from(room * size_of::<libc::statfs>())
        .map_err(|_| io::Error::from_raw_os_error(libc::ENOMEM))?;
    // SAFETY: `table` has room for `bytes` bytes of statfs records.
    let filled = check(unsafe { libc::getfsstat(table.as_mut_ptr(), bytes, libc::MNT_NOWAIT) })
        .map_err(io::Error::from_raw_os_error)?;
    // SAFETY: getfsstat initialised `filled` records, at most `room`.
    unsafe { table.set_len(filled.unsigned_abs() as usize) };
    Ok(table)
}

fn mount_point(st: &libc::statfs) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(c_array_bytes(&st.f_mntonname)))
}

/// The first word of the file-system id: the device number `stat` reports
/// for files on that volume. (In an APFS volume group, files on the sealed
/// system volume report the data volume's device instead.)
fn fs_device(st: &libc::statfs) -> u64 {
    // SAFETY: `fsid_t` is `#[repr(C)] struct { int32_t val[2]; }`; the libc
    // crate only keeps the array private.
    let [device, _kind] = unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(st.f_fsid) };
    u64::from(device.cast_unsigned())
}
