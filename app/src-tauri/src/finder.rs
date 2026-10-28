//! Big & duplicate file finder. Review only: nothing is pre-ticked, and deletes go to the Recycle Bin.
//! Hard exclusions come from the user's CLAUDE.md and are checked again, path by path, before anything is deleted.
use crate::jobs::Progress;
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;

const HIDDEN: u32 = 0x2;
const SYSTEM: u32 = 0x4;
const NOT_LOCAL: u32 = 0x0040_0000 | 0x0004_0000 | 0x0000_1000;
const DUP_MIN: u64 = 1024 * 1024;
const INSTALLER_MIN: u64 = 20 * 1024 * 1024;
const INSTALLER_AGE: Duration = Duration::from_secs(60 * 24 * 3600);

/// A folder holding any of these is a code project and is never entered.
const PROJECT_MARKERS: [&str; 9] = [".git", "package.json", "pyproject.toml", "node_modules", "Cargo.toml", "go.mod", "requirements.txt", ".venv", "venv"];
/// Salon Shaila business files, never touched.
pub(crate) static CLIENT_WORK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)salon|shaila|invoice").unwrap());
/// The CLAUDE.md "Keep" list.
pub(crate) static KEEP: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"(?i)^(3utools|3uairpalyer|3uairplayer)|^cowork data 2026-10-11\.zip$|^physics student book 2(_text)?\.pdf$").unwrap()
});
static INSTALLER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(exe|msi|msix|msixbundle|appx|appxbundle|iso)$").unwrap());

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Item {
  pub path: String,
  pub name: String,
  pub folder: String,
  pub bytes: u64,
  pub modified: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DupGroup {
  pub bytes: u64,
  pub files: Vec<Item>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Found {
  pub large: Vec<Item>,
  pub installers: Vec<Item>,
  pub dups: Vec<DupGroup>,
  pub roots: Vec<String>,
  pub scanned: u64,
  pub skipped_projects: Vec<String>,
}

/// The user's own settings on top of the hard rules: the large-file size and extra exclusions (matched anywhere in the
/// path, ignoring case).
#[derive(Clone)]
pub struct Rules {
  pub large: u64,
  pub exclusions: Vec<String>,
}

impl Default for Rules {
  fn default() -> Self {
    Rules { large: 500 * 1024 * 1024, exclusions: Vec::new() }
  }
}

impl Rules {
  pub fn new(large_mb: u64, exclusions: &[String]) -> Rules {
    Rules { large: large_mb * 1024 * 1024, exclusions: exclusions.iter().map(|e| e.to_lowercase()).filter(|e| !e.is_empty()).collect() }
  }

  pub(crate) fn excluded(&self, path: &Path) -> bool {
    if self.exclusions.is_empty() {
      return false;
    }
    let p = path.to_string_lossy().to_lowercase();
    self.exclusions.iter().any(|e| p.contains(e.as_str()))
  }
}

pub(crate) fn is_project(dir: &Path) -> bool {
  PROJECT_MARKERS.iter().any(|m| dir.join(m).exists())
}

fn ms(t: Option<SystemTime>) -> u64 {
  t.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

struct Seen {
  path: PathBuf,
  bytes: u64,
  modified: Option<SystemTime>,
  root: usize,
}

fn walk(dir: &Path, root: usize, rules: &Rules, out: &mut Vec<Seen>, skipped: &mut Vec<String>, progress: &Progress) {
  if is_project(dir) {
    skipped.push(dir.to_string_lossy().into_owned());
    return;
  }
  for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
    let name = entry.file_name().to_string_lossy().into_owned();
    let Ok(ft) = entry.file_type() else { continue };
    let Ok(md) = entry.metadata() else { continue };
    if ft.is_symlink() || md.file_attributes() & (HIDDEN | SYSTEM) != 0 || CLIENT_WORK.is_match(&name) || rules.excluded(&entry.path()) {
      continue;
    }
    if ft.is_dir() {
      walk(&entry.path(), root, rules, out, skipped, progress);
    } else if md.file_attributes() & NOT_LOCAL == 0 && !name.eq_ignore_ascii_case("desktop.ini") && !KEEP.is_match(&name) {
      out.push(Seen { path: entry.path(), bytes: md.len(), modified: md.modified().ok(), root });
      if out.len() % 2000 == 0 {
        progress.set(format!("Looked at {} files", out.len()));
      }
    }
  }
}

fn item(s: &Seen) -> Item {
  Item {
    path: s.path.to_string_lossy().into_owned(),
    name: s.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    folder: s.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
    bytes: s.bytes,
    modified: ms(s.modified),
  }
}

fn hash(path: &Path, limit: Option<usize>) -> Option<[u8; 32]> {
  let mut f = fs::File::open(path).ok()?;
  let mut h = blake3::Hasher::new();
  let mut buf = vec![0u8; 1 << 16];
  let mut left = limit.unwrap_or(usize::MAX);
  while left > 0 {
    let take = buf.len().min(left);
    let n = f.read(&mut buf[..take]).ok()?;
    if n == 0 {
      break;
    }
    h.update(&buf[..n]);
    left -= n;
  }
  Some(*h.finalize().as_bytes())
}

/// Old installers are only looked for in a root named Downloads.
pub fn scan(roots: &[PathBuf], rules: &Rules, progress: &Progress) -> Found {
  let mut seen = Vec::new();
  let mut skipped = Vec::new();
  for (i, r) in roots.iter().enumerate() {
    progress.set(format!("Looking in {}", r.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
    walk(r, i, rules, &mut seen, &mut skipped, progress);
  }

  let mut large: Vec<Item> = seen.iter().filter(|s| s.bytes >= rules.large).map(item).collect();
  let downloads: Vec<usize> = (0..roots.len()).filter(|i| roots[*i].file_name().is_some_and(|n| n.eq_ignore_ascii_case("Downloads"))).collect();
  large.sort_by(|a, b| b.bytes.cmp(&a.bytes));
  let now = SystemTime::now();
  let mut installers: Vec<Item> = seen
    .iter()
    .filter(|s| downloads.contains(&s.root) && s.bytes >= INSTALLER_MIN && INSTALLER.is_match(&s.path.to_string_lossy()))
    .filter(|s| s.modified.and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age >= INSTALLER_AGE))
    .map(item)
    .collect();
  installers.sort_by(|a, b| b.bytes.cmp(&a.bytes));

  // Duplicates: same size, then same first 64 KB, then same full content.
  let mut by_size: HashMap<u64, Vec<&Seen>> = HashMap::new();
  for s in seen.iter().filter(|s| s.bytes >= DUP_MIN) {
    by_size.entry(s.bytes).or_default().push(s);
  }
  let candidates: Vec<Vec<&Seen>> = by_size.into_values().filter(|v| v.len() > 1).collect();
  let total = candidates.len();
  let mut dups = Vec::new();
  for (i, same_size) in candidates.into_iter().enumerate() {
    if i % 20 == 0 {
      progress.set(format!("Comparing possible duplicates ({i} of {total})"));
    }
    let mut by_head: HashMap<[u8; 32], Vec<&Seen>> = HashMap::new();
    for s in same_size {
      if let Some(h) = hash(&s.path, Some(1 << 16)) {
        by_head.entry(h).or_default().push(s);
      }
    }
    for same_head in by_head.into_values().filter(|v| v.len() > 1) {
      let mut by_full: HashMap<[u8; 32], Vec<&Seen>> = HashMap::new();
      for s in same_head {
        if let Some(h) = hash(&s.path, None) {
          by_full.entry(h).or_default().push(s);
        }
      }
      for mut group in by_full.into_values().filter(|v| v.len() > 1) {
        // Likely original first: oldest, then shortest name.
        group.sort_by(|a, b| a.modified.cmp(&b.modified).then(a.path.as_os_str().len().cmp(&b.path.as_os_str().len())));
        dups.push(DupGroup { bytes: group[0].bytes, files: group.iter().map(|s| item(s)).collect() });
      }
    }
  }
  dups.sort_by(|a, b| (b.bytes * (b.files.len() as u64 - 1)).cmp(&(a.bytes * (a.files.len() as u64 - 1))));
  dups.truncate(200);

  Found {
    large,
    installers,
    dups,
    roots: roots.iter().map(|r| r.to_string_lossy().into_owned()).collect(),
    scanned: seen.len() as u64,
    skipped_projects: skipped,
  }
}

/// The deletion gate: inside a scanned root, and every rule above still holds for this exact path.
pub fn allowed(path: &Path, roots: &[PathBuf], rules: &Rules) -> Result<(), String> {
  let Some(root) = roots.iter().find(|r| path.starts_with(r)) else { return Err("outside the scanned folders".into()) };
  if rules.excluded(path) {
    return Err("on your exclusions list".into());
  }
  let md = fs::symlink_metadata(path).map_err(|_| "no longer exists".to_string())?;
  if !md.is_file() || md.file_type().is_symlink() {
    return Err("not a regular file".into());
  }
  if md.file_attributes() & (HIDDEN | SYSTEM) != 0 {
    return Err("hidden or system file".into());
  }
  let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
  if KEEP.is_match(&name) || name.eq_ignore_ascii_case("desktop.ini") {
    return Err("on your Keep list".into());
  }
  let mut dir = path.parent();
  while let Some(d) = dir {
    if is_project(d) {
      return Err("inside a code project".into());
    }
    if CLIENT_WORK.is_match(&d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) || CLIENT_WORK.is_match(&name) {
      return Err("Salon Shaila work".into());
    }
    if d == root.as_path() {
      break;
    }
    dir = d.parent();
  }
  Ok(())
}

/// Recycle Bin size limit for the volume holding `path`, from Explorer's own settings; 0 when the bin is turned off.
pub fn bin_capacity(path: &Path) -> u64 {
  use winreg::{enums::HKEY_CURRENT_USER, RegKey};
  let root = path.components().next().map(|c| format!("{}\\", c.as_os_str().to_string_lossy())).unwrap_or_else(|| "C:\\".into());
  let (total, _) = crate::storage::space(&root);
  let fallback = total / 20;
  let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
  let mut name = [0u16; 64];
  if unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(w.as_ptr()), &mut name) }.is_err() {
    return fallback;
  }
  let vol = String::from_utf16_lossy(&name[..name.iter().position(|c| *c == 0).unwrap_or(0)]);
  let Some(guid) = vol.find('{').and_then(|a| vol.find('}').map(|b| vol[a..=b].to_string())) else { return fallback };
  let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{guid}")) else {
    return fallback;
  };
  if key.get_value::<u32, _>("NukeOnDelete").unwrap_or(0) == 1 {
    return 0;
  }
  key.get_value::<u32, _>("MaxCapacity").map(|mb| mb as u64 * 1024 * 1024).unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn keep_list_and_client_work() {
    for keep in ["3uTools_v3.12.exe", "3uAirpalyer_setup.exe", "Cowork data 2026-10-11.zip", "Physics Student Book 2.pdf", "Physics Student Book 2_text.pdf"] {
      assert!(KEEP.is_match(keep), "{keep}");
    }
    assert!(!KEEP.is_match("Physics Student Book 3.pdf"));
    assert!(CLIENT_WORK.is_match("Salon Shaila"));
    assert!(CLIENT_WORK.is_match("Invoice-0042.pdf"));
  }

  #[test]
  fn deletion_gate() {
    let base = std::env::temp_dir().join(format!("shimmy-finder-test-{}", std::process::id()));
    let proj = base.join("site");
    let shop = base.join("Salon Shaila");
    for d in [&proj, &shop] {
      fs::create_dir_all(d).unwrap();
    }
    fs::write(proj.join("package.json"), "{}").unwrap();
    for (p, body) in [(base.join("movie.mp4"), "x"), (proj.join("big.bin"), "x"), (shop.join("logo.png"), "x"), (base.join("Cowork data 2026-10-11.zip"), "x")] {
      fs::write(p, body).unwrap();
    }
    let roots = vec![base.clone()];
    assert!(allowed(&base.join("movie.mp4"), &roots, &Rules::default()).is_ok());
    assert_eq!(allowed(&proj.join("big.bin"), &roots, &Rules::default()), Err("inside a code project".into()));
    assert_eq!(allowed(&shop.join("logo.png"), &roots, &Rules::default()), Err("Salon Shaila work".into()));
    assert_eq!(allowed(&base.join("Cowork data 2026-10-11.zip"), &roots, &Rules::default()), Err("on your Keep list".into()));
    assert!(allowed(Path::new(r"C:\Windows\notepad.exe"), &roots, &Rules::default()).is_err());
    assert_eq!(allowed(&base.join("movie.mp4"), &roots, &Rules::new(500, &["MOVIE".into()])), Err("on your exclusions list".into()));
    let _ = fs::remove_dir_all(&base);
  }
}
