//! The two traits the app implements. The engine calls them on its own
//! threads, never on the caller's and never while it holds a lock of its
//! own; an implementation must return quickly.

use std::path::Path;
use std::sync::Arc;

use metla_core::clean;
use metla_core::engine::{self, Changed};

/// Told that something changed, with a coarse topic and no data: the app
/// then asks for what it shows. Merged or lost notifications do no harm.
#[uniffi::export(with_foreign)]
pub trait EngineObserver: Send + Sync {
    fn changed(&self, topic: Changed);
}

/// What moving one path to the Trash came to.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TrashOutcome {
    Moved,
    Refused { reason: String },
}

/// Moves one path to the Trash (the app: `FileManager.trashItem`).
#[uniffi::export(with_foreign)]
pub trait TrashHost: Send + Sync {
    fn trash(&self, path: String) -> TrashOutcome;
}

/// The app's observer, as the engine calls it.
pub struct Observer(pub Arc<dyn EngineObserver>);

impl engine::EngineObserver for Observer {
    fn changed(&self, what: Changed) {
        self.0.changed(what);
    }
}

/// The app's Trash, as the cleaner calls it.
pub struct Trash(pub Arc<dyn TrashHost>);

impl clean::TrashHost for Trash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        match self.0.trash(path.to_string_lossy().into_owned()) {
            TrashOutcome::Moved => Ok(()),
            TrashOutcome::Refused { reason } => Err(reason),
        }
    }
}
