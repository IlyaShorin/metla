//! Worktree statuses for lists. On the real disk there are hundreds of
//! linked worktrees and each costs three Git calls, so they are computed
//! in the background after a scan — two at a time, at background
//! priority, abandoned when a newer request comes — kept per picture, and
//! served with the candidate page. They are for display only: a preview
//! and an execution compute a worktree's status again.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use super::view::Removability;
use crate::domain::{Candidate, DirId, UnixTime, WorktreeStatus};
use crate::fs::background_thread;
use crate::git::{Git, WorktreeSettings, inspect_worktree};
use crate::index::PictureId;

/// Statuses handed to the list at a time.
const CHUNK: usize = 8;
/// Git calls at a time.
const AT_ONCE: usize = 2;

/// Statuses computed for one picture.
#[derive(Default)]
struct Known {
    picture: Option<PictureId>,
    by_dir: BTreeMap<DirId, WorktreeStatus>,
}

/// A request: these worktrees of this picture.
struct Job {
    generation: u64,
    picture: PictureId,
    worktrees: Vec<Candidate>,
    now: UnixTime,
    settings: WorktreeSettings,
}

pub struct Statuses {
    known: Mutex<Known>,
    jobs: Mutex<Option<Sender<Job>>>,
    generation: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Statuses {
    /// Starts the background worker; `ready` is called after each batch.
    pub fn start(git: Git, ready: impl Fn() + Send + 'static) -> (Arc<Statuses>, JoinHandle<()>) {
        let (jobs, queue) = mpsc::channel();
        let statuses = Arc::new(Statuses {
            known: Mutex::default(),
            jobs: Mutex::new(Some(jobs)),
            generation: AtomicU64::new(0),
        });
        let worker = Arc::clone(&statuses);
        let thread = thread::Builder::new()
            .name("metla-worktrees".into())
            .spawn(move || worker.serve(&git, queue, &ready))
            .unwrap_or_else(|e| panic!("cannot start the worktree worker: {e}"));
        (statuses, thread)
    }

    /// What a list shows for the worktree `id` of `picture`.
    pub fn removability(&self, picture: PictureId, id: DirId) -> Removability {
        let known = lock(&self.known);
        let status = known
            .by_dir
            .get(&id)
            .filter(|_| known.picture == Some(picture));
        match status {
            None => Removability::Unknown,
            Some(WorktreeStatus::Removable { branch }) => Removability::Removable {
                branch: branch.clone(),
            },
            Some(WorktreeStatus::Blocked(reasons)) => Removability::Blocked(reasons.clone()),
        }
    }

    /// Computes the statuses of `worktrees` of `picture` again, in the
    /// background. A request for another picture forgets the old one's;
    /// an earlier request still running is abandoned.
    pub fn compute(
        &self,
        picture: PictureId,
        worktrees: Vec<Candidate>,
        now: UnixTime,
        settings: WorktreeSettings,
    ) {
        {
            let mut known = lock(&self.known);
            if known.picture != Some(picture) {
                *known = Known {
                    picture: Some(picture),
                    by_dir: BTreeMap::new(),
                };
            }
            for worktree in &worktrees {
                known.by_dir.remove(&worktree.id);
            }
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let job = Job {
            generation,
            picture,
            worktrees,
            now,
            settings,
        };
        if let Some(jobs) = lock(&self.jobs).as_ref() {
            let _ = jobs.send(job);
        }
    }

    /// Stops the worker after the status it is computing.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        lock(&self.jobs).take();
    }

    fn serve(&self, git: &Git, queue: Receiver<Job>, ready: &dyn Fn()) {
        background_thread();
        for job in queue {
            self.run(git, &job, ready);
        }
    }

    fn current(&self, job: &Job) -> bool {
        self.generation.load(Ordering::Acquire) == job.generation
    }

    fn run(&self, git: &Git, job: &Job, ready: &dyn Fn()) {
        for chunk in job.worktrees.chunks(CHUNK) {
            if !self.current(job) {
                return;
            }
            let found = inspect(git, chunk, job);
            let mut known = lock(&self.known);
            if known.picture != Some(job.picture) {
                return;
            }
            known.by_dir.extend(found);
            drop(known);
            ready();
        }
    }
}

/// The statuses of `chunk`, at most [`AT_ONCE`] at a time.
fn inspect(git: &Git, chunk: &[Candidate], job: &Job) -> Vec<(DirId, WorktreeStatus)> {
    let next = AtomicUsize::new(0);
    let found = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..AT_ONCE {
            scope.spawn(|| {
                background_thread();
                while let Some(c) = chunk.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let status = inspect_worktree(git, &c.path, c.activity, job.now, &job.settings);
                    lock(&found).push((c.id, status));
                }
            });
        }
    });
    found.into_inner().unwrap_or_else(PoisonError::into_inner)
}
