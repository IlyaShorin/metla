use std::ffi::{CStr, OsStr, OsString};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
use std::ptr::NonNull;

use super::bulk::BulkCall;
use super::sys::{c_name, checked, errno, meta_of, retry_interrupted};
use crate::domain::{Entry, EntryKind, EntryMeta, Errno};

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

/// A small file is opened without following a symlink and without waiting
/// on a FIFO; anything but a regular file is then refused.
const OPEN_FILE: libc::c_int =
    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;

/// An open directory. Children are opened relative to it.
#[derive(Debug)]
pub struct DirHandle(OwnedFd);

impl DirHandle {
    /// Opens a directory by path (the scan root, or a directory to list).
    pub fn open(path: &Path) -> Result<DirHandle, ListError> {
        let path = c_name(path.as_os_str()).map_err(ListError::from_errno)?;
        // SAFETY: `path` is a valid NUL-terminated string for the call.
        adopt(checked(|| unsafe { libc::open(path.as_ptr(), OPEN_DIR) })).map(DirHandle)
    }

    /// Opens the child directory `name` without following a symlink.
    pub fn open_child(&self, name: &OsStr) -> Result<DirHandle, ListError> {
        self.open_at(name, OPEN_DIR | libc::O_NOFOLLOW)
    }

    /// The directory's own metadata.
    pub fn meta(&self) -> Result<EntryMeta, ListError> {
        stat_of(&self.0)
    }

    /// The raw descriptor, for calls that read through it.
    pub(super) fn raw(&self) -> libc::c_int {
        self.0.as_raw_fd()
    }

    /// The directory once more, on a descriptor of its own.
    pub fn reopen(&self) -> Result<DirHandle, ListError> {
        self.open_at(OsStr::new("."), OPEN_DIR)
    }

    /// The metadata of the entry `name`, without following a symlink.
    pub fn entry(&self, name: &OsStr) -> Result<EntryMeta, ListError> {
        self.stat_entry(name)
    }

    /// Removes the entry `name`: unlinks a file, a symlink (never its
    /// target) or another non-directory, or removes an empty directory.
    pub fn remove(&self, name: &OsStr, kind: EntryKind) -> Result<(), Errno> {
        let flags = match kind {
            EntryKind::Dir => libc::AT_REMOVEDIR,
            _ => 0,
        };
        let name = c_name(name).map_err(Errno)?;
        // SAFETY: the descriptor is open and `name` is NUL-terminated.
        checked(|| unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), flags) })
            .map(|_| ())
            .map_err(Errno)
    }

    /// At most `limit` bytes from the start of the regular file `name` in
    /// this directory. Used only for tiny files a rule inspects (a `.git`
    /// pointer file, a `CACHEDIR.TAG`).
    pub fn read_head(&self, name: &OsStr, limit: usize) -> Result<Vec<u8>, ListError> {
        let file = self.open_fd(name, OPEN_FILE)?;
        if stat_of(&file)?.kind != EntryKind::File {
            return Err(ListError::Io(Errno(libc::EFTYPE)));
        }
        let mut head = vec![0u8; limit];
        let filled = fill(&file, &mut head).map_err(ListError::from_errno)?;
        head.truncate(filled);
        Ok(head)
    }

    fn open_at(&self, name: &OsStr, flags: libc::c_int) -> Result<DirHandle, ListError> {
        self.open_fd(name, flags).map(DirHandle)
    }

    fn open_fd(&self, name: &OsStr, flags: libc::c_int) -> Result<OwnedFd, ListError> {
        let name = c_name(name).map_err(ListError::from_errno)?;
        // SAFETY: the descriptor is open and `name` is NUL-terminated.
        let fd = checked(|| unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), flags) });
        adopt(fd)
    }

    fn stat_entry(&self, name: &OsStr) -> Result<EntryMeta, ListError> {
        let name = c_name(name).map_err(ListError::from_errno)?;
        let mut st = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the descriptor is open, `name` is NUL-terminated and `st`
        // is writable stat storage.
        checked(|| unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                st.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        })
        .map_err(ListError::from_errno)?;
        // SAFETY: fstatat succeeded, so it initialised `st`.
        Ok(meta_of(unsafe { st.assume_init_ref() }))
    }
}

/// The metadata of `path` itself, without following a final symlink and
/// without needing to read it.
pub fn entry_at(path: &Path) -> Result<EntryMeta, ListError> {
    let path = c_name(path.as_os_str()).map_err(ListError::from_errno)?;
    let mut st = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `path` is NUL-terminated and `st` is writable stat storage.
    checked(|| unsafe { libc::lstat(path.as_ptr(), st.as_mut_ptr()) })
        .map_err(ListError::from_errno)?;
    // SAFETY: lstat succeeded, so it initialised `st`.
    Ok(meta_of(unsafe { st.assume_init_ref() }))
}

fn adopt(fd: Result<libc::c_int, i32>) -> Result<OwnedFd, ListError> {
    let fd = fd.map_err(ListError::from_errno)?;
    // SAFETY: `fd` was just returned by a successful open and nothing else
    // owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Reads from the start of `file` until `buffer` is full or the file ends;
/// returns how many bytes were read. A full buffer reads as the end.
fn fill(file: &OwnedFd, buffer: &mut [u8]) -> Result<usize, i32> {
    let mut filled = 0;
    loop {
        let rest = &mut buffer[filled..];
        // SAFETY: the descriptor is open and `rest` is writable for its
        // whole length.
        let read = retry_interrupted(|| {
            let n = unsafe { libc::read(file.as_raw_fd(), rest.as_mut_ptr().cast(), rest.len()) };
            usize::try_from(n).map_err(|_| errno())
        })?;
        if read == 0 {
            return Ok(filled);
        }
        filled += read;
    }
}

/// The metadata of an open descriptor.
fn stat_of(fd: &OwnedFd) -> Result<EntryMeta, ListError> {
    let mut st = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the descriptor is open and `st` is writable stat storage.
    checked(|| unsafe { libc::fstat(fd.as_raw_fd(), st.as_mut_ptr()) })
        .map_err(ListError::from_errno)?;
    // SAFETY: fstat succeeded, so it initialised `st`.
    Ok(meta_of(unsafe { st.assume_init_ref() }))
}

/// Every entry of `dir` except `.` and `..`, with metadata read without
/// following symlinks. An entry removed while the listing runs is left
/// out. Uses `getattrlistbulk`, and the classic listing where the file
/// system does not support it.
pub fn list_dir(dir: &DirHandle) -> Result<Vec<Entry>, ListError> {
    collect(|sink| visit_dir(dir, sink))
}

/// [`list_dir`] with `call` in place of `getattrlistbulk`.
pub fn list_dir_with(dir: &DirHandle, call: BulkCall) -> Result<Vec<Entry>, ListError> {
    collect(|sink| visit_with(dir, call, sink))
}

/// [`list_dir`] handing each entry to `sink` as it is read, so a
/// directory of any size is listed in bounded memory.
pub fn visit_dir(dir: &DirHandle, sink: &mut dyn FnMut(Entry)) -> Result<(), ListError> {
    visit_with(dir, super::bulk::getattrlistbulk, sink)
}

fn visit_with(
    dir: &DirHandle,
    call: BulkCall,
    sink: &mut dyn FnMut(Entry),
) -> Result<(), ListError> {
    if super::bulk::visit(dir, call, sink)? == super::bulk::Listing::Unsupported {
        visit_classic(dir, sink)?;
    }
    Ok(())
}

/// [`list_dir`] with `readdir` and one `fstatat` per entry.
pub fn list_dir_classic(dir: &DirHandle) -> Result<Vec<Entry>, ListError> {
    collect(|sink| visit_classic(dir, sink))
}

/// What `visit` hands over, in a list.
fn collect(
    visit: impl FnOnce(&mut dyn FnMut(Entry)) -> Result<(), ListError>,
) -> Result<Vec<Entry>, ListError> {
    let mut entries = Vec::new();
    visit(&mut |entry| entries.push(entry))?;
    Ok(entries)
}

fn visit_classic(dir: &DirHandle, sink: &mut dyn FnMut(Entry)) -> Result<(), ListError> {
    let stream = DirStream::open(dir)?;
    while let Some(name) = stream.next_name()? {
        entry_of(dir, name)?.map(&mut *sink);
    }
    Ok(())
}

/// The entry for `name`, or nothing for `.`, `..` and a name that is gone.
pub(super) fn entry_of(dir: &DirHandle, name: OsString) -> Result<Option<Entry>, ListError> {
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
        retry_interrupted(|| self.read_name()).map_err(ListError::from_errno)
    }

    fn read_name(&self) -> Result<Option<OsString>, i32> {
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

fn end_of_stream(errno: i32) -> Result<Option<OsString>, i32> {
    match errno {
        0 => Ok(None),
        failure => Err(failure),
    }
}

impl Drop for DirStream {
    fn drop(&mut self) {
        // SAFETY: the stream is open and is closed exactly once, here.
        unsafe { libc::closedir(self.0.as_ptr()) };
    }
}
