//! Long-running work (disk scans, cleanups, DISM) runs on its own thread; the page polls its status.
use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Serialize)]
pub struct Job<T> {
  pub running: bool,
  pub progress: String,
  pub result: Option<T>,
  pub error: Option<String>,
  /// When the result was produced (ms since epoch).
  pub at: u64,
}

impl<T> Default for Job<T> {
  fn default() -> Self {
    Job { running: false, progress: String::new(), result: None, error: None, at: 0 }
  }
}

pub type Slot<T> = Arc<Mutex<Job<T>>>;

/// Reports progress into the slot from inside the work.
#[derive(Clone)]
pub struct Progress(Arc<dyn Fn(String) + Send + Sync>);

impl Progress {
  pub fn new(f: impl Fn(String) + Send + Sync + 'static) -> Progress {
    Progress(Arc::new(f))
  }

  pub fn set(&self, text: impl Into<String>) {
    (self.0)(text.into())
  }
}

/// Starts `work` unless this job is already running. Returns false if it was.
pub fn start<T: Send + 'static>(slot: &Slot<T>, work: impl FnOnce(Progress) -> Result<T, String> + Send + 'static) -> bool {
  {
    let mut j = slot.lock().unwrap();
    if j.running {
      return false;
    }
    j.running = true;
    j.error = None;
    j.progress = "Starting".into();
  }
  let s2 = slot.clone();
  let progress = Progress(Arc::new(move |t| s2.lock().unwrap().progress = t));
  let slot = slot.clone();
  std::thread::spawn(move || {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(progress)))
      .unwrap_or_else(|_| Err("Something went wrong during the scan.".into()));
    let mut j = slot.lock().unwrap();
    j.running = false;
    j.progress.clear();
    match r {
      Ok(v) => {
        j.result = Some(v);
        j.at = crate::store::now_ms();
      }
      Err(e) => j.error = Some(e),
    }
  });
  true
}

/// Hands a job a fresh result from elsewhere. A scan of its own that is still running stays marked as running, so a
/// second one cannot start beside it.
pub fn set_result<T>(slot: &Slot<T>, result: T) {
  let mut j = slot.lock().unwrap();
  j.result = Some(result);
  j.error = None;
  j.at = crate::store::now_ms();
}

/// Drops a finished job's result (to free memory); a running job is left alone.
pub fn clear<T>(slot: &Slot<T>) {
  let mut j = slot.lock().unwrap();
  if !j.running {
    *j = Job::default();
  }
}
