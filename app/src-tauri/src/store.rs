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

const DELETE: u32 = 0x0001_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

/// A path's text, the same whichever way Windows or Rust wrote it, for comparing.
fn path_key(p: &str) -> String {
  p.strip_prefix(r"\\?\").unwrap_or(p).trim_end_matches('\\').to_lowercase()
}

fn final_path(f: &fs::File) -> Option<String> {
  use windows::Win32::Foundation::HANDLE;
  use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
  let mut buf = vec![0u16; 4096];
  let n = unsafe { GetFinalPathNameByHandleW(HANDLE(f.as_raw_handle()), &mut buf, FILE_NAME_NORMALIZED) } as usize;
  (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

/// The folder's own long name, without following any link in it (8.3 names such as STUDIO~1 are spelled out).
fn long_dir(dir: &Path) -> Option<String> {
  use std::os::windows::ffi::OsStrExt;
  use windows::core::PCWSTR;
  use windows::Win32::Storage::FileSystem::GetLongPathNameW;
  let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
  let mut buf = vec![0u16; 4096];
  let n = unsafe { GetLongPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buf)) } as usize;
  (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

fn delete_on_close(f: &fs::File) -> bool {
  use windows::Win32::Foundation::HANDLE;
  use windows::Win32::Storage::FileSystem::{FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO};
  let gone = FILE_DISPOSITION_INFO { DeleteFile: true.into() };
  unsafe { SetFileInformationByHandle(HANDLE(f.as_raw_handle()), FileDispositionInfo, &gone as *const _ as *const _, std::mem::size_of_val(&gone) as u32).is_ok() }
}

/// A new file in a folder the user can write to, such as Downloads. Shimmy is administrator there, so it never opens
/// what is already at that name: that could be a link to a system file. Refuses an existing file or link, and removes
/// and refuses a file that a link anywhere in the folder's path sent somewhere else.
pub fn create_new(dir: &Path, name: &str) -> std::io::Result<fs::File> {
  const GENERIC_WRITE: u32 = 0x4000_0000;
  let f = fs::OpenOptions::new().write(true).create_new(true).access_mode(GENERIC_WRITE | DELETE).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(dir.join(name))?;
  let wanted = long_dir(dir).map(|d| path_key(&format!("{}\\{name}", d.trim_end_matches('\\'))));
  if wanted.is_some() && final_path(&f).map(|p| path_key(&p)) == wanted {
    return Ok(f);
  }
  discard(&f);
  Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "that folder leads somewhere else"))
}

/// Removes a file `create_new` made, through its handle.
pub fn discard(f: &fs::File) {
  delete_on_close(f);
}

/// `create_new`, then the whole content.
pub fn write_new(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
  create_new(dir, name)?.write_all(bytes)
}

/// Opens what is at this exact path, a link itself rather than what it points to, when it really is below `anchor`: the
/// path's text must be where Windows says the file is, so a link in any folder on the way, a `..` or an 8.3 name is
/// refused. `anchor` is a folder Shimmy trusts, and may itself lead elsewhere (Downloads moved to D:).
fn open_exact(anchor: &Path, path: &Path, access: u32) -> Option<fs::File> {
  let rel = path.strip_prefix(anchor).ok()?;
  if !rel.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
    return None;
  }
  let wanted = fs::canonicalize(anchor).ok()?.join(rel);
  let f = fs::OpenOptions::new().access_mode(access).share_mode(7).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).open(path).ok()?;
  (final_path(&f).map(|p| path_key(&p)) == Some(path_key(&wanted.to_string_lossy()))).then_some(f)
}

/// Whether this exact file or folder is really below `anchor`, and is not itself a link. Cloud placeholders are fine.
pub fn really_at(anchor: &Path, path: &Path) -> bool {
  open_exact(anchor, path, FILE_READ_ATTRIBUTES).is_some_and(|f| f.metadata().is_ok_and(|m| !m.file_type().is_symlink()))
}

/// Deletes one file for good, through a handle that was checked with `open_exact`, so nothing can be swapped in between
/// the check and the delete. A read-only file has the flag cleared first. False when it was refused or is in use.
pub fn delete_exact(anchor: &Path, path: &Path) -> bool {
  use windows::Win32::Foundation::HANDLE;
  use windows::Win32::Storage::FileSystem::{FileBasicInfo, SetFileInformationByHandle, FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO};
  let Some(f) = open_exact(anchor, path, DELETE | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES) else { return false };
  if !f.metadata().is_ok_and(|m| m.is_file()) {
    return false;
  }
  if delete_on_close(&f) {
    return true;
  }
  let plain = FILE_BASIC_INFO { FileAttributes: FILE_ATTRIBUTE_NORMAL.0, ..Default::default() };
  let cleared = unsafe { SetFileInformationByHandle(HANDLE(f.as_raw_handle()), FileBasicInfo, &plain as *const _ as *const _, std::mem::size_of_val(&plain) as u32).is_ok() };
  cleared && delete_on_close(&f)
}

/// Replaces a file in one step, so a crash or a full disk leaves the old copy and never half of a new one. The temporary
/// file is new each time and never opened through something already there.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
  let mut tmp = path.as_os_str().to_os_string();
  tmp.push(".tmp");
  let tmp = PathBuf::from(tmp);
  let _ = fs::remove_file(&tmp);
  let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
  f.write_all(bytes)?;
  f.sync_all()?;
  drop(f);
  fs::rename(&tmp, path)
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

  /// A folder junction, which any program can make without administrator rights.
  fn junction(link: &Path, target: &Path) {
    let ok = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(link).arg(target).stdout(std::process::Stdio::null()).status().unwrap().success();
    assert!(ok, "could not make a junction");
  }

  #[test]
  fn paths_compare_however_they_are_written() {
    assert_eq!(path_key(r"\\?\C:\Users\X\Report.txt"), path_key(r"c:\users\x\report.TXT"));
    assert_ne!(path_key(r"\\?\C:\Windows\System32\Report.txt"), path_key(r"C:\Users\x\Downloads\Report.txt"));
  }

  #[test]
  fn a_link_in_the_folder_cannot_send_a_new_file_elsewhere() {
    let (base, away) = (scratch("link-base"), scratch("link-away"));
    let link = base.join("Downloads");
    junction(&link, &away);
    assert!(write_new(&link, "report.txt", b"x").is_err(), "the junction is refused");
    assert!(!away.join("report.txt").exists(), "and the stray file is removed");
    write_new(&base, "here.txt", b"x").unwrap();
    let _ = (fs::remove_dir(&link), fs::remove_dir_all(&base), fs::remove_dir_all(&away));
  }

  #[test]
  fn the_exact_path_gate_sees_through_links() {
    let (base, away) = (scratch("exact-base"), scratch("exact-away"));
    fs::create_dir_all(base.join("real")).unwrap();
    fs::write(base.join("real").join("a.txt"), b"x").unwrap();
    fs::write(away.join("victim.txt"), b"precious").unwrap();
    junction(&base.join("link"), &away);
    assert!(really_at(&base, &base.join("real")));
    assert!(really_at(&base, &base.join("real").join("a.txt")));
    assert!(!really_at(&base, &base.join("link")), "a link at the end");
    assert!(!really_at(&base, &base.join("link").join("victim.txt")), "a link on the way");
    assert!(!really_at(&base, &base.join("real").join("..").join("real").join("a.txt")), "a .. in the path");
    assert!(!really_at(&base, &base.join("nothing")));
    assert!(!delete_exact(&base, &base.join("link").join("victim.txt")));
    assert_eq!(fs::read(away.join("victim.txt")).unwrap(), b"precious");
    assert!(really_at(&away, &away.join("victim.txt")), "the same file is fine from its own folder");
    let _ = (fs::remove_dir(base.join("link")), fs::remove_dir_all(&base), fs::remove_dir_all(&away));
  }

  #[test]
  fn delete_exact_removes_plain_and_read_only_files() {
    let d = scratch("delete");
    let (a, b) = (d.join("a.txt"), d.join("b.txt"));
    fs::write(&a, b"x").unwrap();
    fs::write(&b, b"x").unwrap();
    let mut ro = fs::metadata(&b).unwrap().permissions();
    ro.set_readonly(true);
    fs::set_permissions(&b, ro).unwrap();
    assert!(delete_exact(&d, &a) && !a.exists());
    assert!(delete_exact(&d, &b) && !b.exists());
    assert!(!delete_exact(&d, &d), "a folder is not deleted this way");
    let _ = fs::remove_dir_all(&d);
  }

  #[test]
  fn atomic_writes_replace_the_old_file() {
    let d = scratch("atomic");
    let f = d.join("settings.json");
    write_atomic(&f, b"one").unwrap();
    write_atomic(&f, b"two").unwrap();
    assert_eq!(fs::read(&f).unwrap(), b"two");
    assert!(!d.join("settings.json.tmp").exists());
    let _ = fs::remove_dir_all(&d);
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
