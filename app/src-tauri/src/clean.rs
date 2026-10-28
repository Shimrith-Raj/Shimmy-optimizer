//! Safe clean: temp files, caches, crash dumps, Windows Update downloads, browser caches, the Recycle Bin, and the
//! DISM component store. Everything is measured first; nothing runs until the user ticks it and presses Clean.
//! These are caches that regrow, so they are deleted outright; the file finder (finder.rs) is what uses the Recycle Bin.
use crate::jobs::Progress;
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};
use windows::core::{w, PCWSTR};
use windows::Win32::System::Services::{
  CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatus, StartServiceW, SC_HANDLE, SC_MANAGER_CONNECT,
  SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS, SERVICE_START, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STOP, SERVICE_STOPPED,
  SERVICE_RUNNING,
};
use windows::Win32::UI::Shell::{SHEmptyRecycleBinW, SHQueryRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHQUERYRBINFO};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CatScan {
  pub id: &'static str,
  pub title: &'static str,
  pub why: &'static str,
  pub bytes: u64,
  pub files: u64,
  pub default_on: bool,
  /// Why it cannot be cleaned right now (for example, the browser is open).
  pub blocked: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatResult {
  pub id: &'static str,
  pub title: &'static str,
  pub freed: u64,
  pub files: u64,
  pub skipped: u64,
  pub note: Option<String>,
}

struct Def {
  id: &'static str,
  title: &'static str,
  why: &'static str,
  default_on: bool,
}

const DEFS: [Def; 10] = [
  Def { id: "user-temp", title: "Your temporary files", why: "Leftovers from installers and apps. Only files older than a day are removed, so nothing in use is touched.", default_on: true },
  Def { id: "win-temp", title: "Windows temporary files", why: "Windows' own temp folder. Same one-day rule.", default_on: true },
  Def { id: "thumbs", title: "Thumbnail cache", why: "Picture previews in File Explorer. Windows rebuilds them as you browse. Files Explorer has open are skipped.", default_on: true },
  Def { id: "dumps", title: "Crash dumps and error reports", why: "Records left after apps or Windows crashed. Only useful to a support engineer.", default_on: true },
  Def { id: "delivery", title: "Delivery Optimization cache", why: "Update pieces Windows keeps to share with other PCs. Safe to clear.", default_on: true },
  Def { id: "wu", title: "Windows Update downloads", why: "Update installers that were already applied. Windows Update is paused for a moment while they are removed.", default_on: true },
  Def { id: "brave", title: "Brave cache", why: "Cached web pages and images only. History, passwords and logins stay. Brave must be closed.", default_on: true },
  Def { id: "chrome", title: "Chrome cache", why: "Cached web pages and images only. History, passwords and logins stay. Chrome must be closed.", default_on: true },
  Def { id: "edge", title: "Edge cache", why: "Cached web pages and images only. History, passwords and logins stay. Edge must be closed, including its background process.", default_on: true },
  Def { id: "recycle", title: "Recycle Bin", why: "Empties it for good. Off by default, because it is your undo for files you deleted.", default_on: false },
];

pub fn is_category(id: &str) -> bool {
  DEFS.iter().any(|d| d.id == id)
}

/// (id, title) of every category, for the Settings page.
pub fn categories() -> Vec<(&'static str, &'static str)> {
  DEFS.iter().map(|d| (d.id, d.title)).collect()
}

fn env_path(var: &str) -> PathBuf {
  PathBuf::from(std::env::var_os(var).unwrap_or_default())
}

/// Folder to clean, with a minimum file age and an optional file-name filter.
struct Target {
  dir: PathBuf,
  min_age: Duration,
  recursive: bool,
  only: Option<fn(&str) -> bool>,
}

fn dir(p: PathBuf) -> Target {
  Target { dir: p, min_age: Duration::ZERO, recursive: true, only: None }
}

fn browser(id: &str) -> Option<(&'static str, PathBuf)> {
  let local = env_path("LOCALAPPDATA");
  match id {
    "brave" => Some(("brave.exe", local.join(r"BraveSoftware\Brave-Browser\User Data"))),
    "chrome" => Some(("chrome.exe", local.join(r"Google\Chrome\User Data"))),
    "edge" => Some(("msedge.exe", local.join(r"Microsoft\Edge\User Data"))),
    _ => None,
  }
}

fn targets(id: &str) -> Vec<Target> {
  let day = Duration::from_secs(24 * 3600);
  let local = env_path("LOCALAPPDATA");
  let windows = env_path("SystemRoot");
  let program_data = env_path("ProgramData");
  match id {
    "user-temp" => vec![Target { min_age: day, ..dir(std::env::temp_dir()) }],
    "win-temp" => vec![Target { min_age: day, ..dir(windows.join("Temp")) }],
    "thumbs" => vec![Target {
      recursive: false,
      only: Some(|n| {
        let n = n.to_ascii_lowercase();
        (n.starts_with("thumbcache_") || n.starts_with("iconcache_")) && n.ends_with(".db")
      }),
      ..dir(local.join(r"Microsoft\Windows\Explorer"))
    }],
    "dumps" => vec![
      dir(local.join("CrashDumps")),
      dir(windows.join("Minidump")),
      dir(local.join(r"Microsoft\Windows\WER\ReportArchive")),
      dir(local.join(r"Microsoft\Windows\WER\ReportQueue")),
      dir(program_data.join(r"Microsoft\Windows\WER\ReportArchive")),
      dir(program_data.join(r"Microsoft\Windows\WER\ReportQueue")),
      Target { recursive: false, only: Some(|n| n.eq_ignore_ascii_case("MEMORY.DMP")), ..dir(windows.clone()) },
    ],
    "delivery" => vec![dir(windows.join(r"ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache"))],
    "wu" => vec![dir(windows.join(r"SoftwareDistribution\Download"))],
    _ => match browser(id) {
      Some((_, data)) => {
        let mut out = vec![dir(data.join("ShaderCache")), dir(data.join("GrShaderCache"))];
        for entry in fs::read_dir(&data).into_iter().flatten().flatten() {
          let name = entry.file_name().to_string_lossy().into_owned();
          if name == "Default" || name.starts_with("Profile ") || name == "Guest Profile" {
            for cache in ["Cache", "Code Cache", "GPUCache"] {
              out.push(dir(entry.path().join(cache)));
            }
          }
        }
        out
      }
      None => vec![],
    },
  }
}

fn old_enough(md: &fs::Metadata, min_age: Duration) -> bool {
  if min_age.is_zero() {
    return true;
  }
  let newest = [md.modified().ok(), md.created().ok()].into_iter().flatten().max();
  newest.and_then(|t| SystemTime::now().duration_since(t).ok()).map(|age| age >= min_age).unwrap_or(false)
}

/// Visits matching files under a target: (path, bytes).
fn files(t: &Target, out: &mut Vec<(PathBuf, u64)>) {
  fn visit(t: &Target, d: &Path, out: &mut Vec<(PathBuf, u64)>) {
    for entry in fs::read_dir(d).into_iter().flatten().flatten() {
      let Ok(ft) = entry.file_type() else { continue };
      if ft.is_symlink() {
        continue;
      }
      if ft.is_dir() {
        if t.recursive {
          visit(t, &entry.path(), out);
        }
        continue;
      }
      let name = entry.file_name().to_string_lossy().into_owned();
      if t.only.is_some_and(|f| !f(&name)) {
        continue;
      }
      if let Ok(md) = entry.metadata() {
        if old_enough(&md, t.min_age) {
          out.push((entry.path(), md.len()));
        }
      }
    }
  }
  visit(t, &t.dir, out);
}

/// Bytes and items in the Recycle Bin, across all drives.
pub fn recycle_bin() -> (u64, u64) {
  let mut info = SHQUERYRBINFO { cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32, ..Default::default() };
  unsafe {
    let _ = SHQueryRecycleBinW(None, &mut info);
  }
  (info.i64Size.max(0) as u64, info.i64NumItems.max(0) as u64)
}

fn blocked(id: &str, running: &HashSet<String>) -> Option<String> {
  let (exe, _) = browser(id)?;
  running.contains(exe).then(|| {
    let name = DEFS.iter().find(|d| d.id == id).map(|d| d.title.trim_end_matches(" cache")).unwrap_or(id);
    format!("{name} is open. Close it to clean its cache.")
  })
}

pub fn scan(running: &HashSet<String>, progress: &Progress) -> Vec<CatScan> {
  DEFS.iter()
    .map(|d| {
      progress.set(format!("Measuring {}", d.title.to_lowercase()));
      let (bytes, files_n) = if d.id == "recycle" {
        recycle_bin()
      } else {
        let mut list = Vec::new();
        for t in targets(d.id) {
          files(&t, &mut list);
        }
        (list.iter().map(|f| f.1).sum(), list.len() as u64)
      };
      CatScan { id: d.id, title: d.title, why: d.why, bytes, files: files_n, default_on: d.default_on, blocked: blocked(d.id, running) }
    })
    .collect()
}

fn delete_files(list: &[(PathBuf, u64)]) -> (u64, u64, u64) {
  let (mut freed, mut done, mut skipped) = (0, 0, 0);
  for (path, bytes) in list {
    let mut ok = fs::remove_file(path).is_ok();
    if !ok {
      // Read-only files need the flag cleared first; files in use stay and are counted as skipped.
      if let Ok(md) = fs::metadata(path) {
        let mut perm = md.permissions();
        if perm.readonly() {
          #[allow(clippy::permissions_set_readonly_false)]
          perm.set_readonly(false);
          ok = fs::set_permissions(path, perm).is_ok() && fs::remove_file(path).is_ok();
        }
      }
    }
    if ok {
      freed += bytes;
      done += 1;
    } else {
      skipped += 1;
    }
  }
  (freed, done, skipped)
}

/// Removes now-empty folders below (never including) the target folder.
fn prune_empty(d: &Path) {
  for entry in fs::read_dir(d).into_iter().flatten().flatten() {
    if entry.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) {
      prune_empty(&entry.path());
      let _ = fs::remove_dir(entry.path());
    }
  }
}

pub fn hidden(cmd: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
  Command::new(cmd).args(args).stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW).output()
}

/// Closes a service or Service Control Manager handle when it goes out of scope.
struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
  fn drop(&mut self) {
    unsafe {
      let _ = CloseServiceHandle(self.0);
    }
  }
}

/// Opens a service through the Service Control Manager with the given access.
fn open_service(name: PCWSTR, access: u32) -> Option<ScHandle> {
  unsafe {
    let scm = ScHandle(OpenSCManagerW(None, None, SC_MANAGER_CONNECT).ok()?);
    Some(ScHandle(OpenServiceW(scm.0, name, access).ok()?))
  }
}

/// Polls until the service reaches the state, for at most ten seconds.
fn wait_for_state(svc: &ScHandle, state: SERVICE_STATUS_CURRENT_STATE) {
  for _ in 0..50 {
    let mut st = SERVICE_STATUS::default();
    if unsafe { QueryServiceStatus(svc.0, &mut st) }.is_ok_and(|_| st.dwCurrentState == state) {
      return;
    }
    std::thread::sleep(Duration::from_millis(200));
  }
}

/// Stops a service and waits until it has stopped. Errors are ignored: a service that is already stopped is fine.
fn stop_service(name: PCWSTR) {
  let Some(svc) = open_service(name, SERVICE_STOP | SERVICE_QUERY_STATUS) else { return };
  let mut st = SERVICE_STATUS::default();
  if unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st) }.is_ok() {
    wait_for_state(&svc, SERVICE_STOPPED);
  }
}

/// Starts a service and waits until it is running. Errors are ignored: a service that is already running is fine.
fn start_service(name: PCWSTR) {
  let Some(svc) = open_service(name, SERVICE_START | SERVICE_QUERY_STATUS) else { return };
  if unsafe { StartServiceW(svc.0, None) }.is_ok() {
    wait_for_state(&svc, SERVICE_RUNNING);
  }
}

pub fn run(ids: &[String], running: &HashSet<String>, progress: &Progress) -> Vec<CatResult> {
  let mut results = Vec::new();
  for d in DEFS.iter().filter(|d| ids.iter().any(|i| i == d.id)) {
    progress.set(format!("Cleaning {}", d.title.to_lowercase()));
    let r = |freed, files, skipped, note: Option<String>| CatResult { id: d.id, title: d.title, freed, files, skipped, note };
    if let Some(why) = blocked(d.id, running) {
      results.push(r(0, 0, 0, Some(why)));
      continue;
    }
    match d.id {
      "recycle" => {
        let (bytes, n) = recycle_bin();
        let ok = unsafe { SHEmptyRecycleBinW(None, None, SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND).is_ok() };
        results.push(if ok || n == 0 { r(bytes, n, 0, None) } else { r(0, 0, n, Some("Windows did not empty the Recycle Bin.".into())) });
      }
      _ => {
        let wu = d.id == "wu";
        if wu {
          stop_service(w!("wuauserv"));
        }
        let ts = targets(d.id);
        let mut list = Vec::new();
        ts.iter().for_each(|t| files(t, &mut list));
        let (freed, done, skipped) = delete_files(&list);
        for t in ts.iter().filter(|t| t.recursive && t.min_age.is_zero()) {
          prune_empty(&t.dir);
        }
        if wu {
          start_service(w!("wuauserv"));
        }
        results.push(r(freed, done, skipped, (skipped > 0).then(|| format!("{skipped} files were in use and left in place."))));
      }
    }
  }
  results
}

// ---------- DISM component store ----------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Dism {
  pub store_size: Option<String>,
  pub backups: Option<String>,
  pub cache: Option<String>,
  pub reclaimable_packages: Option<u32>,
  pub recommended: Option<bool>,
  pub cleaned: bool,
  pub summary: String,
}

fn field(text: &str, label: &str) -> Option<String> {
  text.lines().find(|l| l.trim_start().starts_with(label)).and_then(|l| l.split_once(':')).map(|(_, v)| v.trim().to_string())
}

pub fn parse_analyze(text: &str) -> Dism {
  Dism {
    store_size: field(text, "Actual Size of Component Store"),
    backups: field(text, "Backups and Disabled Features"),
    cache: field(text, "Cache and Temporary Data"),
    reclaimable_packages: field(text, "Number of Reclaimable Packages").and_then(|v| v.parse().ok()),
    recommended: field(text, "Component Store Cleanup Recommended").map(|v| v.eq_ignore_ascii_case("yes")),
    cleaned: false,
    summary: String::new(),
  }
}

/// Runs DISM and reports its percentage as progress (it redraws a bar with carriage returns).
fn dism(args: &[&str], progress: &Progress, label: &str) -> Result<String, String> {
  let mut child = Command::new("dism.exe")
    .args(args)
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .stdin(Stdio::null())
    .creation_flags(CREATE_NO_WINDOW)
    .spawn()
    .map_err(|e| format!("Could not start DISM: {e}"))?;
  let mut out = child.stdout.take().unwrap();
  let mut text = String::new();
  let mut buf = [0u8; 512];
  while let Ok(n) = out.read(&mut buf) {
    if n == 0 {
      break;
    }
    text.push_str(&String::from_utf8_lossy(&buf[..n]));
    if let Some(pct) = text.rsplit('%').nth(1).and_then(|s| s.rsplit(|c: char| !(c.is_ascii_digit() || c == '.')).next()).filter(|s| !s.is_empty()) {
      progress.set(format!("{label} {pct}%"));
    }
  }
  let status = child.wait().map_err(|e| e.to_string())?;
  if status.success() { Ok(text) } else { Err(text.lines().rev().find(|l| l.contains("Error")).unwrap_or("DISM failed.").trim().to_string()) }
}

pub fn dism_analyze(progress: &Progress) -> Result<Dism, String> {
  progress.set("Analysing the component store");
  let text = dism(&["/Online", "/English", "/Cleanup-Image", "/AnalyzeComponentStore"], progress, "Analysing")?;
  let mut d = parse_analyze(&text);
  d.summary = match d.recommended {
    Some(true) => format!("Clean-up recommended. About {} of old update backups can go.", d.backups.clone().unwrap_or_else(|| "some space".into())),
    Some(false) => "Nothing to clean up; Windows says the component store is already tidy.".into(),
    None => "DISM gave an answer Shimmy could not read.".into(),
  };
  Ok(d)
}

pub fn dism_cleanup(progress: &Progress) -> Result<Dism, String> {
  dism(&["/Online", "/English", "/Cleanup-Image", "/StartComponentCleanup"], progress, "Cleaning up")?;
  let mut d = dism_analyze(progress)?;
  d.cleaned = true;
  d.summary = format!("Done. The component store is now {}.", d.store_size.clone().unwrap_or_else(|| "smaller".into()));
  Ok(d)
}

#[cfg(test)]
mod tests {
  #[test]
  fn reads_dism_analysis() {
    let text = "Component Store (WinSxS) information:\r\n\r\nWindows Explorer Reported Size of Component Store : 8.12 GB\r\n\r\nActual Size of Component Store : 7.86 GB\r\n\r\n    Shared with Windows : 5.71 GB\r\n    Backups and Disabled Features : 2.03 GB\r\n    Cache and Temporary Data : 112.40 MB\r\n\r\nDate of Last Cleanup : 2026-09-02 10:14:51\r\n\r\nNumber of Reclaimable Packages : 3\r\nComponent Store Cleanup Recommended : Yes\r\n";
    let d = super::parse_analyze(text);
    assert_eq!(d.store_size.as_deref(), Some("7.86 GB"));
    assert_eq!(d.backups.as_deref(), Some("2.03 GB"));
    assert_eq!(d.reclaimable_packages, Some(3));
    assert_eq!(d.recommended, Some(true));
  }
}
