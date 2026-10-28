//! RAM history (6 h) and the action log, in the app's data folder. Same file formats as RAM Optimizer v0.2, so its
//! history.json and actions-log.csv are imported as they are.
use regex::Regex;
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY_MS: u64 = 6 * 3600 * 1000;

pub struct Store {
  dir: PathBuf,
  history: Mutex<Vec<(u64, u64)>>,
  /// (time ms, load %, CPU °C if known)
  cpu_history: Mutex<Vec<(u64, f32, Option<f32>)>>,
  log_lock: Mutex<()>,
}

#[derive(Serialize)]
pub struct LogRow {
  pub time: String,
  pub action: String,
  pub target: String,
  pub detail: String,
}

pub fn now_ms() -> u64 {
  SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// UTC ISO-8601 like JavaScript's toISOString().
pub fn iso(ms: u64) -> String {
  let (secs, millis) = (ms / 1000, ms % 1000);
  let days = (secs / 86_400) as i64;
  let tod = secs % 86_400;
  // Howard Hinnant's civil_from_days.
  let z = days + 719_468;
  let era = z.div_euclid(146_097);
  let doe = z - era * 146_097;
  let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let d = doy - (153 * mp + 2) / 5 + 1;
  let m = if mp < 10 { mp + 3 } else { mp - 9 };
  let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
  format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z", tod / 3600, tod % 3600 / 60, tod % 60)
}

static CSV_FIELD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""((?:[^"]|"")*)""#).unwrap());

impl Store {
  pub fn open(dir: PathBuf) -> Store {
    let _ = fs::create_dir_all(&dir);
    let history = fs::read_to_string(dir.join("history.json"))
      .ok()
      .and_then(|s| serde_json::from_str::<Vec<(u64, u64)>>(&s).ok())
      .unwrap_or_default();
    let cpu_history = fs::read_to_string(dir.join("cpu-history.json"))
      .ok()
      .and_then(|s| serde_json::from_str::<Vec<(u64, f32, Option<f32>)>>(&s).ok())
      .unwrap_or_default();
    let store = Store { dir, history: Mutex::new(history), cpu_history: Mutex::new(cpu_history), log_lock: Mutex::new(()) };
    store.prune();
    store
  }

  fn prune(&self) {
    let cutoff = now_ms().saturating_sub(HISTORY_MS);
    self.history.lock().unwrap().retain(|p| p.0 >= cutoff);
    self.cpu_history.lock().unwrap().retain(|p| p.0 >= cutoff);
  }

  pub fn sample(&self, used_mb: u64) {
    self.history.lock().unwrap().push((now_ms(), used_mb));
    self.prune();
  }

  pub fn sample_cpu(&self, load: f32, temp: Option<f32>) {
    self.cpu_history.lock().unwrap().push((now_ms(), (load * 10.0).round() / 10.0, temp.map(|t| (t * 10.0).round() / 10.0)));
    self.prune();
  }

  pub fn cpu_history(&self) -> Vec<(u64, f32, Option<f32>)> {
    self.cpu_history.lock().unwrap().clone()
  }

  pub fn history(&self) -> Vec<(u64, u64)> {
    self.history.lock().unwrap().clone()
  }

  pub fn save(&self) {
    let json = serde_json::to_string(&*self.history.lock().unwrap()).unwrap_or_default();
    let _ = fs::write(self.dir.join("history.json"), json);
    let cpu = serde_json::to_string(&*self.cpu_history.lock().unwrap()).unwrap_or_default();
    let _ = fs::write(self.dir.join("cpu-history.json"), cpu);
  }

  fn log_file(&self) -> PathBuf {
    self.dir.join("actions-log.csv")
  }

  pub fn log(&self, action: &str, target: &str, detail: &str) {
    let _guard = self.log_lock.lock().unwrap();
    let path = self.log_file();
    let fresh = !path.exists();
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let line = format!("{},{},{},{}\n", q(&iso(now_ms())), q(action), q(target), q(detail));
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
      let _ = f.write_all(format!("{}{line}", if fresh { "time,action,target,detail\n" } else { "" }).as_bytes());
    }
  }

  pub fn read_log(&self) -> Vec<LogRow> {
    let Ok(text) = fs::read_to_string(self.log_file()) else { return Vec::new() };
    let rows: Vec<&str> = text.trim().lines().skip(1).collect();
    rows.iter().rev().take(100).map(|r| {
      let f: Vec<String> = CSV_FIELD.captures_iter(r).map(|c| c[1].replace("\"\"", "\"")).collect();
      let get = |i: usize| f.get(i).cloned().unwrap_or_default();
      LogRow { time: get(0), action: get(1), target: get(2), detail: get(3) }
    }).collect()
  }

  /// One-time import from RAM Optimizer v0.2. Only runs when Shimmy has no log yet; the old folder is left untouched.
  pub fn import_old(&self, old_data: &Path) -> Option<String> {
    if self.log_file().exists() || !old_data.is_dir() {
      return None;
    }
    let mut imported = Vec::new();
    if let Ok(log) = fs::read_to_string(old_data.join("actions-log.csv")) {
      if fs::write(self.log_file(), log).is_ok() {
        imported.push("action log");
      }
    }
    if let Some(old) = fs::read_to_string(old_data.join("history.json")).ok().and_then(|s| serde_json::from_str::<Vec<(u64, u64)>>(&s).ok()) {
      let mut h = self.history.lock().unwrap();
      h.extend(old);
      h.sort_by_key(|p| p.0);
      h.dedup_by_key(|p| p.0);
      drop(h);
      self.prune();
      self.save();
      imported.push("RAM history");
    }
    (!imported.is_empty()).then(|| imported.join(" and "))
  }
}

#[cfg(test)]
mod tests {
  #[test]
  fn iso_matches_javascript() {
    assert_eq!(super::iso(1_793_116_881_123), "2026-10-27T16:01:21.123Z");
    assert_eq!(super::iso(0), "1970-01-01T00:00:00.000Z");
  }
}
