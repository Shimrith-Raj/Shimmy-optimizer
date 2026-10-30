//! RAM history (6 h) and the action log, in the app's data folder. Same file formats as RAM Optimizer v0.2, so its
//! history.json and actions-log.csv are imported as they are.
use regex::Regex;
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
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

/// One CSV line, every field quoted, as the log and the exports write them.
pub fn csv_row(fields: &[&str]) -> String {
  let q: Vec<String> = fields.iter().map(|s| format!("\"{}\"", s.replace('"', "\"\""))).collect();
  format!("{}\n", q.join(","))
}

/// A cell for a CSV the user opens in Excel. One that starts like a formula (=, +, -, @, a tab or a carriage return)
/// gets a leading apostrophe, which Excel shows as plain text: file names and app names come from the disk and the
/// registry, and quoting alone does not stop Excel from running `=HYPERLINK(...)`.
pub fn export_cell(s: &str) -> String {
  if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() }
}

/// One line of an export: `csv_row`, with every cell made safe for Excel.
pub fn export_row(fields: &[&str]) -> String {
  let safe: Vec<String> = fields.iter().map(|f| export_cell(f)).collect();
  csv_row(&safe.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Same file name as asked for. A link in the folder's path can send a new file somewhere else under another name.
fn same_name(final_path: &str, name: &str) -> bool {
  final_path.rsplit('\\').next().is_some_and(|n| n.eq_ignore_ascii_case(name))
}

fn final_path(f: &fs::File) -> Option<String> {
  use windows::Win32::Foundation::HANDLE;
  use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
  let mut buf = vec![0u16; 4096];
  let n = unsafe { GetFinalPathNameByHandleW(HANDLE(f.as_raw_handle()), &mut buf, FILE_NAME_NORMALIZED) } as usize;
  (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

/// A new file in a folder the user can write to, such as Downloads. Shimmy is administrator there, so it never opens
/// what is already at that name: that could be a link to a system file. Refuses an existing file or link, and removes
/// and refuses a file that a link in the folder's path sent somewhere else.
pub fn create_new(dir: &Path, name: &str) -> std::io::Result<fs::File> {
  use windows::Win32::Foundation::HANDLE;
  use windows::Win32::Storage::FileSystem::{FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO};
  const GENERIC_WRITE: u32 = 0x4000_0000;
  const DELETE: u32 = 0x0001_0000;
  const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
  let f = fs::OpenOptions::new().write(true).create_new(true).access_mode(GENERIC_WRITE | DELETE).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(dir.join(name))?;
  if final_path(&f).is_some_and(|p| same_name(&p, name)) {
    return Ok(f);
  }
  let gone = FILE_DISPOSITION_INFO { DeleteFile: true.into() };
  unsafe {
    let _ = SetFileInformationByHandle(HANDLE(f.as_raw_handle()), FileDispositionInfo, &gone as *const _ as *const _, std::mem::size_of_val(&gone) as u32);
  }
  Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "that folder leads somewhere else"))
}

/// `create_new`, then the whole content.
pub fn write_new(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
  create_new(dir, name)?.write_all(bytes)
}

/// A real file of a sensible size, not a link to one: what Shimmy may copy out of a folder the user can write to, as
/// administrator.
pub fn plain_file(p: &Path) -> bool {
  const REPARSE_POINT: u32 = 0x400;
  const MAX_BYTES: u64 = 256 * 1_048_576;
  fs::symlink_metadata(p).is_ok_and(|m| m.is_file() && m.file_attributes() & REPARSE_POINT == 0 && m.len() <= MAX_BYTES)
}

/// Copies Shimmy's data from the user's folder to the protected one, once: only when the new folder holds none of it.
/// The old copies stay where they are, and are no longer used. Returns how many files came across.
pub fn move_in(from: &Path, to: &Path) -> usize {
  const FILES: [&str; 4] = ["settings.json", "history.json", "cpu-history.json", "actions-log.csv"];
  if FILES.iter().any(|f| to.join(f).exists()) {
    return 0;
  }
  FILES.iter().filter(|f| plain_file(&from.join(f)) && fs::read(from.join(f)).and_then(|b| fs::write(to.join(f), b)).is_ok()).count()
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

  pub fn log_file(&self) -> PathBuf {
    self.dir.join("actions-log.csv")
  }

  pub fn log(&self, action: &str, target: &str, detail: &str) {
    let _guard = self.log_lock.lock().unwrap();
    let path = self.log_file();
    let fresh = !path.exists();
    let line = csv_row(&[&iso(now_ms()), action, target, detail]);
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

  /// The whole log for an export, every cell made safe for Excel (`export_row`). None when nothing has been logged.
  pub fn export_log(&self) -> Option<String> {
    let text = fs::read_to_string(self.log_file()).ok()?;
    let mut lines = text.lines();
    let mut out = format!("{}\n", lines.next()?);
    for r in lines {
      let f: Vec<String> = CSV_FIELD.captures_iter(r).map(|c| c[1].replace("\"\"", "\"")).collect();
      out += &export_row(&f.iter().map(String::as_str).collect::<Vec<_>>());
    }
    Some(out)
  }

  /// One-time import from RAM Optimizer v0.2. Only runs when Shimmy has no log yet; the old folder is left untouched.
  pub fn import_old(&self, old_data: &Path) -> Option<String> {
    if self.log_file().exists() || !old_data.is_dir() {
      return None;
    }
    let mut imported = Vec::new();
    if let Some(log) = Some(old_data.join("actions-log.csv")).filter(|p| plain_file(p)).and_then(|p| fs::read_to_string(p).ok()) {
      if fs::write(self.log_file(), log).is_ok() {
        imported.push("action log");
      }
    }
    if let Some(old) = Some(old_data.join("history.json")).filter(|p| plain_file(p)).and_then(|p| fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str::<Vec<(u64, u64)>>(&s).ok()) {
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
  use super::*;

  fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("shimmy-store-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
  }

  #[test]
  fn new_files_never_open_what_is_already_there() {
    let d = scratch("new");
    write_new(&d, "a.txt", b"first").unwrap();
    assert_eq!(fs::read(d.join("a.txt")).unwrap(), b"first");
    assert_eq!(write_new(&d, "a.txt", b"second").unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(d.join("a.txt")).unwrap(), b"first", "the old file is untouched");
    // Making a link needs Developer Mode or administrator; without either, the rest of this test has nothing to try.
    let victim = d.join("victim.txt");
    fs::write(&victim, b"precious").unwrap();
    if std::os::windows::fs::symlink_file(&victim, d.join("link.txt")).is_ok() {
      assert!(write_new(&d, "link.txt", b"evil").is_err(), "a link at that name");
      assert_eq!(fs::read(&victim).unwrap(), b"precious");
      assert!(!plain_file(&d.join("link.txt")), "a link is not a plain file");
    }
    let _ = fs::remove_dir_all(&d);
  }

  #[test]
  fn a_new_file_must_keep_its_name() {
    assert!(same_name(r"\\?\C:\Users\x\Downloads\Shimmy Report.txt", "shimmy report.txt"));
    assert!(!same_name(r"\\?\C:\Windows\System32\evil.dll", "Shimmy report.txt"));
  }

  #[test]
  fn data_moves_in_once_and_only_as_plain_files() {
    let (from, to) = (scratch("from"), scratch("to"));
    fs::write(from.join("settings.json"), b"{}").unwrap();
    fs::write(from.join("actions-log.csv"), b"log").unwrap();
    fs::write(from.join("other.txt"), b"not ours").unwrap();
    assert_eq!(move_in(&from, &to), 2);
    assert_eq!(fs::read(to.join("settings.json")).unwrap(), b"{}");
    assert!(!to.join("other.txt").exists());
    assert!(from.join("settings.json").exists(), "the old copy stays");
    fs::write(from.join("settings.json"), b"changed").unwrap();
    assert_eq!(move_in(&from, &to), 0, "only once");
    assert_eq!(fs::read(to.join("settings.json")).unwrap(), b"{}");
    assert!(!plain_file(&from), "a folder is not a plain file");
    let _ = (fs::remove_dir_all(&from), fs::remove_dir_all(&to));
  }

  #[test]
  fn iso_matches_javascript() {
    assert_eq!(super::iso(1_793_116_881_123), "2026-10-27T16:01:21.123Z");
    assert_eq!(super::iso(0), "1970-01-01T00:00:00.000Z");
  }

  #[test]
  fn csv_quotes_every_field() {
    assert_eq!(super::csv_row(&["a", "say \"hi\"", "x,y"]), "\"a\",\"say \"\"hi\"\"\",\"x,y\"\n");
  }

  #[test]
  fn exports_never_hold_formulas() {
    assert_eq!(super::export_row(&["=HYPERLINK(\"x\")", "@sum", "+1", "-2", "\tx", "plain", "a=b"]), "\"'=HYPERLINK(\"\"x\"\")\",\"'@sum\",\"'+1\",\"'-2\",\"'\tx\",\"plain\",\"a=b\"\n");
  }
}
