use std::ffi::{CStr, OsStr, OsString};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
use std::ptr::NonNull;

use super::sys::{c_name, check, errno, meta_of};
use crate::domain::{Entry, EntryMeta, Errno};

/// Why a directory could not be opened or listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListError {
    /// `EACCES` / `EPERM`.
    Denied,
    /// `ENOENT` / `ENOTDIR`: it is gone, or is no longer a directory.
    Gone,
    Io(Errno),
}

impl ListError {
    pub fn from_errno(errno: i32) -> ListError {
        match errno {
            libc::EACCES | libc::EPERM => ListError::Denied,
            libc::ENOENT | libc::ENOTDIR => ListError::Gone,
            other => ListError::Io(Errno(other)),
        }
    }
}

const OPEN_DIR: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;

/// An open directory. Children are opened relative to it.
#[derive(Debug)]
pub struct DirHandle(OwnedFd);

impl DirHandle {
    /// Opens a directory by path (the scan root, or a directory to list).
    pub fn open(path: &Path) -> Result<DirHandle, ListError> {
        let path = c_name(path.as_os_str()).map_err(ListError::from_errno)?;
        // SAFETY: `path` is a valid NUL-terminated string for the call.
        let fd = unsafe { libc::open(path.as_ptr(), OPEN_DIR) };
        DirHandle::adopt(fd)
    }

    /// Opens the child directory `name` without following a symlink.
    pub fn open_child(&self, name: &OsStr) -> Result<DirHandle, ListError> {
        self.open_at(name, OPEN_DIR | libc::O_NOFOLLOW)
    }

    /// The directory's own metadata.
    pub fn meta(&self) -> Result<EntryMeta, ListError> {
        let mut st = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the descriptor is open and `st` is writable stat storage.
        check(unsafe { libc::fstat(self.0.as_raw_fd(), st.as_mut_ptr()) })
            .map_err(ListError::from_errno)?;
        // SAFETY: fstat succeeded, so it initialised `st`.
        Ok(meta_of(unsafe { st.assume_init_ref() }))
    }

    fn open_at(&self, name: &OsStr, flags: libc::c_int) -> Result<DirHandle, ListError> {
        let name = c_name(name).map_err(ListError::from_errno)?;
        // SAFETY: the descriptor is open and `name` is NUL-terminated.
        let fd = unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), flags) };
        DirHandle::adopt(fd)
    }

    fn adopt(fd: libc::c_int) -> Result<DirHandle, ListError> {
        let fd = check(fd).map_err(ListError::from_errno)?;
        // SAFETY: `fd` was just returned by a successful open and nothing
        // else owns it.
        Ok(DirHandle(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    fn stat_entry(&self, name: &OsStr) -> Result<EntryMeta, ListError> {
        let name = c_name(name).map_err(ListError::from_errno)?;
        let mut st = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the descriptor is open, `name` is NUL-terminated and `st`
        // is writable stat storage.
        let ret = unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                st.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        check(ret).map_err(ListError::from_errno)?;
        // SAFETY: fstatat succeeded, so it initialised `st`.
        Ok(meta_of(unsafe { st.assume_init_ref() }))
    }
}

/// Every entry of `dir` except `.` and `..`, with metadata read without
/// following symlinks. An entry removed while the listing runs is left out.
pub fn list_dir(dir: &DirHandle) -> Result<Vec<Entry>, ListError> {
    let stream = DirStream::open(dir)?;
    let mut entries = Vec::new();
    while let Some(name) = stream.next_name()? {
        entries.extend(entry_of(dir, name)?);
    }
    Ok(entries)
}

/// The entry for `name`, or nothing for `.`, `..` and a name that is gone.
fn entry_of(dir: &DirHandle, name: OsString) -> Result<Option<Entry>, ListError> {
    if name == "." || name == ".." {
        return Ok(None);
    }
    match dir.stat_entry(&name) {
        Ok(meta) => Ok(Some(Entry { name, meta })),
        Err(ListError::Gone) => Ok(None),
        Err(other) => Err(other),
    }
}

/// A `readdir` stream over its own descriptor, so every listing starts at
/// the beginning and the handle's descriptor stays usable for children.
struct DirStream(NonNull<libc::DIR>);

impl DirStream {
    fn open(dir: &DirHandle) -> Result<DirStream, ListError> {
        let own = dir.open_at(OsStr::new("."), OPEN_DIR)?;
        // SAFETY: `own` is an open directory descriptor; on success the
        // stream takes ownership of it, so it must not be closed here.
        let stream = unsafe { libc::fdopendir(own.0.as_raw_fd()) };
        let stream = NonNull::new(stream).ok_or_else(|| ListError::from_errno(errno()))?;
        std::mem::forget(own);
        Ok(DirStream(stream))
    }

    fn next_name(&self) -> Result<Option<OsString>, ListError> {
        // SAFETY: `__error` returns this thread's errno location; readdir
        // signals end-of-stream versus failure only through errno.
        unsafe { *libc::__error() = 0 };
        // SAFETY: the stream is open and used by this thread only.
        let entry = unsafe { libc::readdir(self.0.as_ptr()) };
        match NonNull::new(entry) {
            // SAFETY: readdir returned a valid entry whose name is
            // NUL-terminated; the bytes are copied before the next call.
            Some(entry) => Ok(Some(OsString::from_vec(
                unsafe { CStr::from_ptr(entry.as_ref().d_name.as_ptr()) }
                    .to_bytes()
                    .to_vec(),
            ))),
            None => end_of_stream(errno()),
        }
    }
}

fn end_of_stream(errno: i32) -> Result<Option<OsString>, ListError> {
    match errno {
        0 => Ok(None),
        failure => Err(ListError::from_errno(failure)),
    }
}

impl Drop for DirStream {
    fn drop(&mut self) {
        // SAFETY: the stream is open and is closed exactly once, here.
        unsafe { libc::closedir(self.0.as_ptr()) };
    }
}
