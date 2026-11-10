//! The FSEvents stream: every foreign declaration the watcher needs
//! (CoreServices, CoreFoundation, libdispatch) is in this file, and so is
//! every `unsafe` block of the module.
//!
//! The stream is directory-level (no per-file events), watches its root
//! itself (`WatchRoot`), defers its events by the latency (no `NoDefer`),
//! and delivers each batch on a serial dispatch queue of its own to the
//! caller's sink.

use std::ffi::{CStr, CString, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::coalescer::Event;
use super::flags::Flags;

type CFIndex = isize;
type CFTypeRef = *const c_void;
type FSEventStreamRef = *mut c_void;
type DispatchQueue = *mut c_void;

/// `FSEventStreamCallback`.
type Callback = extern "C" fn(
    stream: *const c_void,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
);

#[repr(C)]
struct StreamContext {
    version: CFIndex,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

const UTF8: u32 = 0x0800_0100;
const SINCE_NOW: u64 = u64::MAX;
const WATCH_ROOT: u32 = 0x04;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: CFTypeRef,
        callback: Callback,
        context: *const StreamContext,
        paths: CFTypeRef,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(stream: FSEventStreamRef, queue: DispatchQueue);
    fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
    fn FSEventStreamFlushSync(stream: FSEventStreamRef);
    fn FSEventStreamStop(stream: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    fn FSEventStreamRelease(stream: FSEventStreamRef);
    fn FSEventsGetCurrentEventId() -> u64;
    fn FSEventsCopyUUIDForDevice(device: libc::dev_t) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: c_void;
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        string: *const c_char,
        encoding: u32,
    ) -> CFTypeRef;
    fn CFArrayCreate(
        allocator: CFTypeRef,
        values: *const CFTypeRef,
        count: CFIndex,
        callbacks: *const c_void,
    ) -> CFTypeRef;
    fn CFRelease(object: CFTypeRef);
    fn CFUUIDCreateString(allocator: CFTypeRef, uuid: CFTypeRef) -> CFTypeRef;
    fn CFStringGetCString(
        string: CFTypeRef,
        buffer: *mut c_char,
        size: CFIndex,
        encoding: u32,
    ) -> u8;
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> DispatchQueue;
    fn dispatch_sync_f(
        queue: DispatchQueue,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
    fn dispatch_release(object: DispatchQueue);
}

/// Where a stream hands its batches.
type Sink = Box<dyn Fn(Vec<Event>) + Send>;

/// A running FSEvents stream. Dropping it stops the stream; no batch is
/// delivered after the drop returns.
pub struct Stream {
    stream: FSEventStreamRef,
    queue: DispatchQueue,
    sink: *mut Sink,
}

// SAFETY: the stream and its queue are only touched through the
// thread-safe FSEvents and dispatch calls, and the sink is `Send`.
unsafe impl Send for Stream {}

impl Stream {
    /// Watches `root`, replaying from the event after `since` (`None`:
    /// from now on). Batches arrive at most every `latency`.
    pub fn start(
        root: &Path,
        since: Option<u64>,
        latency: Duration,
        sink: impl Fn(Vec<Event>) + Send + 'static,
    ) -> Result<Stream, String> {
        let root_text = CString::new(root.as_os_str().as_bytes())
            .map_err(|_| format!("{} holds a NUL byte", root.display()))?;
        let sink: *mut Sink = Box::into_raw(Box::new(Box::new(sink)));
        let context = StreamContext {
            version: CFIndex::default(),
            info: sink.cast(),
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        // SAFETY: a NUL-terminated string that outlives the call, which
        // copies it.
        let path = unsafe { CFStringCreateWithCString(std::ptr::null(), root_text.as_ptr(), UTF8) };
        // SAFETY: one valid CFString; the array retains it.
        let paths = unsafe {
            CFArrayCreate(
                std::ptr::null(),
                &path,
                1,
                (&raw const kCFTypeArrayCallBacks).cast(),
            )
        };
        // SAFETY: the context, the array and the callback are valid; the
        // stream copies the context and retains the array.
        let stream = unsafe {
            FSEventStreamCreate(
                std::ptr::null(),
                deliver,
                &context,
                paths,
                since.unwrap_or(SINCE_NOW),
                latency.as_secs_f64(),
                WATCH_ROOT,
            )
        };
        // SAFETY: both were created above and are released once.
        unsafe {
            CFRelease(paths);
            CFRelease(path);
        }
        if stream.is_null() {
            // SAFETY: the stream never saw the sink.
            drop(unsafe { Box::from_raw(sink) });
            return Err(format!("no event stream for {}", root.display()));
        }
        // SAFETY: a valid NUL-terminated label; serial queue (no attributes).
        let queue = unsafe { dispatch_queue_create(c"metla-events".as_ptr(), std::ptr::null()) };
        // SAFETY: the stream and the queue are valid; started once.
        let started = unsafe {
            FSEventStreamSetDispatchQueue(stream, queue);
            FSEventStreamStart(stream)
        };
        let running = Stream {
            stream,
            queue,
            sink,
        };
        match started {
            0 => Err(format!(
                "the event stream for {} did not start",
                root.display()
            )),
            _ => Ok(running),
        }
    }

    /// Delivers what the stream holds back now, and waits until it is
    /// delivered.
    pub fn flush(&self) {
        // SAFETY: the stream is valid until the drop.
        unsafe { FSEventStreamFlushSync(self.stream) };
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the stream is stopped, invalidated and released on its
        // own queue, so no callback runs meanwhile or afterwards; then the
        // queue and the sink are released, each once.
        unsafe {
            dispatch_sync_f(self.queue, self.stream, end);
            dispatch_release(self.queue);
            drop(Box::from_raw(self.sink));
        }
    }
}

extern "C" fn end(stream: *mut c_void) {
    // SAFETY: called on the stream's queue with the stream as context.
    unsafe {
        FSEventStreamStop(stream);
        FSEventStreamInvalidate(stream);
        FSEventStreamRelease(stream);
    }
}

extern "C" fn deliver(
    _stream: *const c_void,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
) {
    // SAFETY: FSEvents passes `count` C strings, flags and ids, and the
    // context's info, which is the sink this stream owns.
    let (sink, events) = unsafe {
        let paths = std::slice::from_raw_parts(paths as *const *const c_char, count);
        let paths: Vec<&CStr> = paths.iter().map(|path| CStr::from_ptr(*path)).collect();
        let flags = std::slice::from_raw_parts(flags, count);
        let ids = std::slice::from_raw_parts(ids, count);
        (&*(info as *const Sink), events_of(&paths, flags, ids))
    };
    // A panic must not unwind into the system's code.
    let _ = catch_unwind(AssertUnwindSafe(|| sink(events)));
}

/// The events of one batch as FSEvents hands it over: a path, the flags
/// and the id of each, in the same order. Pure, so it is tested without
/// waiting for a real delivery.
pub fn events_of(paths: &[&CStr], flags: &[u32], ids: &[u64]) -> Vec<Event> {
    paths
        .iter()
        .zip(flags)
        .zip(ids)
        .map(|((path, flags), id)| Event {
            path: PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())),
            flags: Flags::from_bits(*flags),
            id: *id,
        })
        .collect()
}

/// The id of the latest event the system has recorded.
pub fn current_event_id() -> u64 {
    // SAFETY: no arguments, no state.
    unsafe { FSEventsGetCurrentEventId() }
}

/// The UUID FSEvents keeps for the device's event history; it changes
/// when the history is reset. `None` when it has none.
pub fn volume_uuid(device: u64) -> Option<String> {
    let device = libc::dev_t::from_ne_bytes((device as u32).to_ne_bytes());
    // SAFETY: plain value in; a CF object (or null) out, released below.
    let uuid = unsafe { FSEventsCopyUUIDForDevice(device) };
    if uuid.is_null() {
        return None;
    }
    // SAFETY: `uuid` is a valid CFUUID; the string is released below.
    let text = unsafe { CFUUIDCreateString(std::ptr::null(), uuid) };
    let mut buffer = [c_char::default(); 64];
    // SAFETY: the buffer is writable for its length; both objects are
    // released once.
    let ok = unsafe {
        let ok = CFStringGetCString(text, buffer.as_mut_ptr(), buffer.len() as CFIndex, UTF8);
        CFRelease(text);
        CFRelease(uuid);
        ok
    };
    // SAFETY: on success the buffer holds a NUL-terminated string.
    let read = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    (ok != 0).then(|| read.to_string_lossy().into_owned())
}
