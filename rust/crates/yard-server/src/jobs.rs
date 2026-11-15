//! Long work the server does in the background while a page watches how far
//! it has got: a smoothing pass over a big ship takes long enough that doing
//! it in the browser locks the page up.
//!
//! A job reports what it is doing and how far through it is; the page asks
//! now and then, and can ask it to stop. One job runs per design at a time.
//! Finished jobs are kept a while so a page that looks late still learns
//! how it ended.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yard_core::api::{JobState, JobStatus};

/// How long a finished job is remembered, with what it made: a piece made
/// from a model waits this long to be placed.
const KEEP_FINISHED: Duration = Duration::from_secs(60 * 60);

pub struct Job {
    design: String,
    started: Instant,
    progress: Mutex<Progress>,
    stop: AtomicBool,
    /// How it ended, and when.
    end: Mutex<Option<(Instant, JobStatus)>>,
    /// The design the work made, if it makes one.
    made: Mutex<Option<String>>,
    /// What the work made that is kept here rather than saved: a piece to place.
    kept: Mutex<Option<Arc<dyn std::any::Any + Send + Sync>>>,
}

/// Which step the work is at and how far through that it is.
#[derive(Clone, Default)]
struct Progress {
    stage: u32,
    stages: u32,
    what: String,
    done: u64,
    total: u64,
}

impl Job {
    /// Says what the work is doing: step `stage` of `stages`, `done` of
    /// `total` through it. Returns false once it has been asked to stop.
    pub fn report(&self, stage: u32, stages: u32, what: &str, done: u64, total: u64) -> bool {
        let mut progress = self.progress.lock().unwrap();
        if progress.what != what {
            progress.what = what.to_string();
        }
        (progress.stage, progress.stages, progress.done, progress.total) = (stage, stages, done, total);
        !self.stopping()
    }

    /// Keeps what the work made, for later requests to use.
    pub fn keep<T: std::any::Any + Send + Sync>(&self, thing: Arc<T>) {
        *self.kept.lock().unwrap() = Some(thing);
    }

    /// What the work kept, if it kept something of that kind.
    pub fn kept<T: std::any::Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.kept.lock().unwrap().clone()?.downcast::<T>().ok()
    }

    /// Records the design the work made, for the page to open.
    pub fn made(&self, design: &str) {
        *self.made.lock().unwrap() = Some(design.to_string());
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Records how the work ended.
    pub fn finish(&self, state: JobState, message: impl Into<String>, rev: Option<u64>, undo: usize, redo: usize) {
        let p = self.progress.lock().unwrap().clone();
        let design = self.made.lock().unwrap().clone();
        let status = JobStatus {
            state,
            what: p.what,
            stage: p.stage,
            stages: p.stages,
            done: p.done,
            total: p.total,
            seconds: self.started.elapsed().as_secs_f64(),
            message: message.into(),
            rev,
            undo,
            redo,
            design,
        };
        *self.end.lock().unwrap() = Some((Instant::now(), status));
    }

    pub fn status(&self) -> JobStatus {
        if let Some((_, status)) = &*self.end.lock().unwrap() {
            return status.clone();
        }
        let p = self.progress.lock().unwrap().clone();
        JobStatus {
            state: JobState::Running,
            what: p.what,
            stage: p.stage,
            stages: p.stages,
            done: p.done,
            total: p.total,
            seconds: self.started.elapsed().as_secs_f64(),
            message: String::new(),
            rev: None,
            undo: 0,
            redo: 0,
            design: None,
        }
    }

    fn running(&self) -> bool {
        self.end.lock().unwrap().is_none()
    }

    fn forgotten(&self) -> bool {
        self.end.lock().unwrap().as_ref().is_some_and(|(at, _)| at.elapsed() > KEEP_FINISHED)
    }
}

#[derive(Default)]
pub struct Jobs {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}

impl Jobs {
    /// Registers a new job on a design. Refused while another one is running on it.
    pub fn start(&self, design: &str, what: &str) -> Result<(String, Arc<Job>), String> {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.retain(|_, job| !job.forgotten());
        if jobs.values().any(|job| job.design == design && job.running()) {
            return Err("Something else is still being worked out for this design. Wait for it, or stop it, first.".into());
        }
        let id = crate::store::Store::new_id();
        let job = Arc::new(Job {
            design: design.to_string(),
            started: Instant::now(),
            progress: Mutex::new(Progress { what: what.to_string(), ..Progress::default() }),
            stop: AtomicBool::new(false),
            end: Mutex::new(None),
            made: Mutex::new(None),
            kept: Mutex::new(None),
        });
        jobs.insert(id.clone(), job.clone());
        Ok((id, job))
    }

    pub fn get(&self, id: &str) -> Option<Arc<Job>> {
        self.jobs.lock().unwrap().get(id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_job_runs_per_design_and_its_end_is_kept() {
        let jobs = Jobs::default();
        let (id, job) = jobs.start("d1", "Starting").unwrap();
        assert!(jobs.start("d1", "Again").is_err(), "a second job on the same design waits");
        assert!(jobs.start("d2", "Other").is_ok(), "other designs are free");
        assert!(job.report(2, 5, "Bevelling", 10, 100));
        let status = jobs.get(&id).unwrap().status();
        assert_eq!((status.state, status.what.as_str(), status.done, status.total), (JobState::Running, "Bevelling", 10, 100));
        assert_eq!((status.stage, status.stages), (2, 5));
        job.stop();
        assert!(!job.report(2, 5, "Bevelling", 20, 100), "asked to stop, the work hears it");
        job.finish(JobState::Stopped, "Stopped.", None, 0, 0);
        assert_eq!(jobs.get(&id).unwrap().status().state, JobState::Stopped);
        assert!(jobs.start("d1", "Now").is_ok(), "once it ended the design is free again");
    }
}
