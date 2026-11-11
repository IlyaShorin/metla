//! What a file-system event says besides its path: the bits of
//! `FSEventStreamEventFlags` the coalescer acts on. The values are the
//! system's; the type is plain data, so the coalescer stays pure.

/// The flags of one event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Flags(u32);

impl Flags {
    /// Something changed in the directory: re-read it.
    pub const NONE: Flags = Flags(0);
    /// Events below the path were merged: walk all of it.
    pub const MUST_SCAN_SUBDIRS: Flags = Flags(0x01);
    /// The client fell behind and events were lost.
    pub const USER_DROPPED: Flags = Flags(0x02);
    /// The kernel fell behind and events were lost.
    pub const KERNEL_DROPPED: Flags = Flags(0x04);
    /// Event ids wrapped around: the history has a gap.
    pub const EVENT_IDS_WRAPPED: Flags = Flags(0x08);
    /// The replay of the stored history is complete (no change).
    pub const HISTORY_DONE: Flags = Flags(0x10);
    /// The watched root itself was moved, removed or replaced.
    pub const ROOT_CHANGED: Flags = Flags(0x20);
    /// A volume was mounted at the path.
    pub const MOUNT: Flags = Flags(0x40);
    /// A volume was unmounted from the path.
    pub const UNMOUNT: Flags = Flags(0x80);

    pub fn from_bits(bits: u32) -> Flags {
        Flags(bits)
    }

    /// Whether every bit of `other` is set.
    pub fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets of bits.
    pub fn with(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }

    /// Whether the event ids wrapped or the root changed: nothing short
    /// of a full rescan is right after this, wherever the event is.
    pub fn lost_history(self) -> bool {
        let lost = [Flags::EVENT_IDS_WRAPPED, Flags::ROOT_CHANGED];
        lost.iter().any(|flag| self.contains(*flag))
    }

    /// Whether the event's directory must be walked whole: events below
    /// it were coalesced, or lost (FSEvents marks lost events as
    /// informational flags beside `MUST_SCAN_SUBDIRS`, on the directory
    /// they were lost in).
    pub fn must_scan(self) -> bool {
        let walk = [
            Flags::MUST_SCAN_SUBDIRS,
            Flags::USER_DROPPED,
            Flags::KERNEL_DROPPED,
        ];
        walk.iter().any(|flag| self.contains(*flag))
    }

    /// Whether a volume appeared or went at the path.
    pub fn mount_changed(self) -> bool {
        self.contains(Flags::MOUNT) || self.contains(Flags::UNMOUNT)
    }
}
