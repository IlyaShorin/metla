//! Listing a directory with `getattrlistbulk`: the names and metadata of
//! many entries per system call, instead of `readdir` and one `fstatat`
//! per entry. Two kinds of entry report their own attributes rather than
//! what they lead to, and are asked again with `fstatat`, as the classic
//! listing does: a mount point (the bulk call describes the directory
//! underneath) and a firmlink (it describes the link, not its target).
//! So is an entry the bulk call could not describe. Reading a record is
//! pure ([`decode_bulk`]); only the listing makes system calls.

use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::num::NonZeroU64;
use std::os::unix::ffi::OsStrExt;

use super::dir::{DirHandle, ListError, entry_of};
use super::sys::{NANOS, SF_DATALESS, checked};
use crate::domain::{BLOCK_SIZE, Entry, EntryKind, EntryMeta, Stamp, UnixTime};

/// `ATTR_CMN_ERROR`: the entry's own error, if it has one.
const ATTR_CMN_ERROR: libc::attrgroup_t = 0x2000_0000;
/// `SF_FIRMLINK` from `<sys/stat.h>`.
const SF_FIRMLINK: u32 = 0x0080_0000;
/// `VREG`, `VDIR` and `VLNK` of `enum vtype`.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

const COMMON: libc::attrgroup_t = libc::ATTR_CMN_RETURNED_ATTRS
    | libc::ATTR_CMN_NAME
    | ATTR_CMN_ERROR
    | libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_CHGTIME
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID;
const DIRS: libc::attrgroup_t = libc::ATTR_DIR_LINKCOUNT | libc::ATTR_DIR_MOUNTSTATUS;
const FILES: libc::attrgroup_t = libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_DATAALLOCSIZE;

/// Directory and file attributes come as a group only for the objects
/// they apply to; within the groups every requested attribute is packed
/// (`FSOPT_PACK_INVAL_ATTRS`), so the returned set says which are valid.
/// Bytes handed to one call.
const BUFFER: usize = 64 * 1024;

thread_local! {
    static SPACE: RefCell<Vec<u8>> = RefCell::new(vec![u8::default(); BUFFER]);
}

/// How a bulk listing ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Listing {
    /// Every entry was handed on.
    Done,
    /// The file system does not support the bulk call; nothing was
    /// handed on.
    Unsupported,
}

/// The bulk call: fills the buffer with records of the open directory's
/// entries for the request and options, and says how many, or fails with
/// an errno. A parameter, so that a test can answer as a file system
/// without the call does (`ENOTSUP`): APFS, HFS+ and devfs all answer it.
pub type BulkCall = fn(libc::c_int, &mut libc::attrlist, &mut [u8], u64) -> Result<usize, i32>;

/// `getattrlistbulk(2)`, repeated when interrupted.
pub fn getattrlistbulk(
    fd: libc::c_int,
    request: &mut libc::attrlist,
    buffer: &mut [u8],
    options: u64,
) -> Result<usize, i32> {
    // SAFETY: the caller's descriptor is open, the request is a valid
    // attrlist and the buffer is writable for its whole length.
    let count = checked(|| unsafe {
        libc::getattrlistbulk(
            fd,
            std::ptr::from_mut(request).cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            options,
        )
    })?;
    Ok(count as usize)
}

/// Hands every entry of `dir` to `sink`, listed with `call`, unless its
/// file system does not support the bulk call.
pub(super) fn visit(
    dir: &DirHandle,
    call: BulkCall,
    sink: &mut dyn FnMut(Entry),
) -> Result<Listing, ListError> {
    let own = dir.reopen()?;
    let mut request = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        commonattr: COMMON,
        dirattr: DIRS,
        fileattr: FILES,
        // SAFETY: an all-zero attrlist is valid: no attributes.
        ..unsafe { std::mem::zeroed() }
    };
    SPACE.with_borrow_mut(|buffer| {
        loop {
            let options = u64::from(libc::FSOPT_PACK_INVAL_ATTRS);
            let count = match call(own.raw(), &mut request, buffer, options) {
                Ok(count) => count,
                Err(libc::ENOTSUP) => return Ok(Listing::Unsupported),
                Err(errno) => return Err(ListError::from_errno(errno)),
            };
            if count == 0 {
                return Ok(Listing::Done);
            }
            let mut at = 0;
            for _ in 0..count {
                let record = &buffer[at..];
                at += read_u32(record, 0) as usize;
                entry(dir, record)?.map(&mut *sink);
            }
        }
    })
}

/// Reads a record's attributes in the order they are packed.
struct Cursor<'r> {
    record: &'r [u8],
    at: usize,
}

impl Cursor<'_> {
    fn u32(&mut self) -> u32 {
        let value = read_u32(self.record, self.at);
        self.at += 4;
        value
    }

    fn u64(&mut self) -> u64 {
        let value = read_u64(self.record, self.at);
        self.at += 8;
        value
    }

    fn i64(&mut self) -> i64 {
        self.u64().cast_signed()
    }
}

/// The entry a record describes; `None` for an entry that is gone.
fn entry(dir: &DirHandle, record: &[u8]) -> Result<Option<Entry>, ListError> {
    match decode_bulk(record) {
        Bulk::Described(entry) => Ok(Some(entry)),
        Bulk::AskAgain(name) => entry_of(dir, name),
    }
}

/// What one record of `getattrlistbulk` says about an entry.
#[derive(Debug, PartialEq, Eq)]
pub enum Bulk {
    /// Everything a listing gives.
    Described(Entry),
    /// To be asked with `fstatat`, as the classic listing does: a mount
    /// point, a firmlink, or an entry with an error of its own or with an
    /// attribute the call could not give.
    AskAgain(OsString),
}

/// Reads one record of the bulk call, as the listing requests it: the
/// record's length; the returned attribute sets (common, volume,
/// directory, file, fork); the entry's error; a reference to its name;
/// device, object type, modification and change times, flags and file id;
/// then link count and mount status for a directory, or link count and
/// allocated size for anything else.
pub fn decode_bulk(record: &[u8]) -> Bulk {
    let mut cursor = Cursor { record, at: 4 };
    let returned = [
        cursor.u32(),
        cursor.u32(),
        cursor.u32(),
        cursor.u32(),
        cursor.u32(),
    ];
    let [common, _, dirs, files, _] = returned;
    let error = cursor.u32();
    let name = name(record, cursor.at);
    cursor.at += 8;
    if error != 0 {
        return Bulk::AskAgain(name);
    }
    let device = u64::from(cursor.u32());
    let kind = cursor.u32();
    let mtime = UnixTime(cursor.i64());
    cursor.at += 8;
    let changed = Stamp(cursor.i64() * NANOS + cursor.i64());
    let flags = cursor.u32();
    let inode = cursor.u64();
    let (kind, links, blocks, valid) = match kind {
        VDIR => directory(&mut cursor, dirs),
        _ => file(&mut cursor, kind, files),
    };
    let link = flags & SF_FIRMLINK != 0;
    if !valid || link || common & COMMON != COMMON {
        return Bulk::AskAgain(name);
    }
    let meta = EntryMeta {
        kind,
        blocks,
        links,
        device,
        inode,
        mtime,
        changed,
        dataless: flags & SF_DATALESS != 0,
    };
    Bulk::Described(Entry { name, meta })
}

/// A directory's group: valid unless it is a mount point (then the record
/// describes the directory underneath). A directory's own link count and
/// blocks are not used; they are one and none, as in the classic listing.
fn directory(
    cursor: &mut Cursor<'_>,
    returned: libc::attrgroup_t,
) -> (EntryKind, NonZeroU64, u64, bool) {
    cursor.u32();
    let mount = cursor.u32() & libc::DIR_MNTSTATUS_MNTPOINT != 0;
    let valid = returned & DIRS == DIRS && !mount;
    (EntryKind::Dir, NonZeroU64::MIN, u64::default(), valid)
}

/// Anything else's group: its links and allocated blocks.
fn file(
    cursor: &mut Cursor<'_>,
    kind: u32,
    returned: libc::attrgroup_t,
) -> (EntryKind, NonZeroU64, u64, bool) {
    let links = NonZeroU64::new(u64::from(cursor.u32())).unwrap_or(NonZeroU64::MIN);
    let blocks = cursor.u64() / BLOCK_SIZE;
    let kind = match kind {
        VREG => EntryKind::File,
        VLNK => EntryKind::Symlink,
        _ => EntryKind::Other,
    };
    (kind, links, blocks, returned & FILES == FILES)
}

/// The name the `attrreference_t` at `at` points at, without its NUL
/// byte.
fn name(record: &[u8], at: usize) -> OsString {
    let offset = read_u32(record, at) as usize;
    let length = read_u32(record, at + 4) as usize;
    let start = at + offset;
    let bytes = &record[start..start + length];
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    OsStr::from_bytes(bytes).to_owned()
}

fn read_u32(record: &[u8], at: usize) -> u32 {
    let bytes = record[at..at + 4].try_into().unwrap_or_default();
    u32::from_ne_bytes(bytes)
}

fn read_u64(record: &[u8], at: usize) -> u64 {
    let bytes = record[at..at + 8].try_into().unwrap_or_default();
    u64::from_ne_bytes(bytes)
}
