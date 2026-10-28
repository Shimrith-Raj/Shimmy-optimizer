//! The Storage page: drives, SSD health (from the sensor sidecar), and what fills C:.
use crate::jobs::Progress;
use crate::sensors::Reading;
use serde::Serialize;
use std::collections::HashMap;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};

const DRIVE_FIXED: u32 = 3;
/// Cloud placeholders and offline files take no space on this disk.
const NOT_LOCAL: u32 = 0x0040_0000 | 0x0004_0000 | 0x0000_1000;

fn wide(s: &str) -> Vec<u16> {
  s.encode_utf16().chain(Some(0)).collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
  pub letter: String,
  pub label: String,
  pub total: u64,
  pub free: u64,
}

pub fn space(root: &str) -> (u64, u64) {
  let (mut free, mut total) = (0u64, 0u64);
  let w = wide(root);
  unsafe {
    let _ = GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), None, Some(&mut total), Some(&mut free));
  }
  (total, free)
}

pub fn drives() -> Vec<Drive> {
  let mask = unsafe { GetLogicalDrives() };
  (0..26u8)
    .filter(|i| mask & (1 << i) != 0)
    .filter_map(|i| {
      let root = format!("{}:\\", (b'A' + i) as char);
      let w = wide(&root);
      if unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) } != DRIVE_FIXED {
        return None;
      }
      let mut label = [0u16; 64];
      unsafe {
        let _ = GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut label), None, None, None, None);
      }
      let label = String::from_utf16_lossy(&label[..label.iter().position(|c| *c == 0).unwrap_or(0)]);
      let (total, free) = space(&root);
      Some(Drive { letter: root[..2].to_string(), label, total, free })
    })
    .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
  pub name: String,
  pub temp_c: Option<f64>,
  pub warn_c: Option<f64>,
  pub life_pct: Option<f64>,
  pub spare_pct: Option<f64>,
  pub read_gb: Option<f64>,
  pub written_gb: Option<f64>,
}

pub fn health(rows: &[Reading]) -> Vec<Health> {
  let mut names: Vec<&str> = Vec::new();
  for r in rows.iter().filter(|r| r.hw_type == "Storage") {
    if !names.contains(&r.hw.as_str()) {
      names.push(&r.hw);
    }
  }
  names
    .into_iter()
    .map(|name| {
      let v = |kind: &str, sensor: &str| rows.iter().find(|r| r.hw == name && r.kind == kind && r.sensor == sensor).and_then(|r| r.value);
      Health {
        name: name.to_string(),
        temp_c: v("Temperature", "Composite Temperature").or_else(|| v("Temperature", "Temperature")),
        warn_c: v("Temperature", "Warning Temperature"),
        life_pct: v("Level", "Life"),
        spare_pct: v("Level", "Available Spare"),
        read_gb: v("Data", "Data Read"),
        written_gb: v("Data", "Data Written"),
      }
    })
    .collect()
}

// ---------- What fills C: ----------

#[derive(Serialize, Clone)]
pub struct Category {
  pub id: &'static str,
  pub name: &'static str,
  pub bytes: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
  pub total: u64,
  pub used: u64,
  pub categories: Vec<Category>,
  pub profile: String,
  pub files: u64,
}

/// Folder sizes inside the user profile, for drilling down.
#[derive(Default)]
pub struct Tree {
  pub total: HashMap<PathBuf, u64>,
  pub direct: HashMap<PathBuf, u64>,
  pub children: HashMap<PathBuf, Vec<PathBuf>>,
}

/// Bytes of files directly inside each folder under `root`. Folders are read in parallel; on Windows a directory
/// listing already carries each file's size, so no extra call per file is needed.
fn walk(root: &Path, counter: &AtomicU64, progress: &Progress) -> HashMap<PathBuf, u64> {
  let direct: std::sync::Mutex<HashMap<PathBuf, u64>> = Default::default();
  fn visit<'s>(scope: &rayon::Scope<'s>, dir: PathBuf, direct: &'s std::sync::Mutex<HashMap<PathBuf, u64>>, counter: &'s AtomicU64, progress: &'s Progress) {
    let (mut bytes, mut files) = (0u64, 0u64);
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
      let Ok(ft) = entry.file_type() else { continue };
      // Junctions such as "Application Data" report as links and are not followed.
      if ft.is_symlink() {
        continue;
      }
      if ft.is_dir() {
        scope.spawn(move |s| visit(s, entry.path(), direct, counter, progress));
      } else if let Ok(md) = entry.metadata() {
        if md.file_attributes() & NOT_LOCAL == 0 {
          bytes += md.len();
          files += 1;
        }
      }
    }
    if files > 0 {
      direct.lock().unwrap().insert(dir, bytes);
      let before = counter.fetch_add(files, Ordering::Relaxed);
      if before / 25_000 != (before + files) / 25_000 {
        progress.set(format!("Looked at {} files", before + files));
      }
    }
  }
  rayon::scope(|s| visit(s, root.to_path_buf(), &direct, counter, progress));
  direct.into_inner().unwrap()
}

fn fold(direct: &HashMap<PathBuf, u64>, root: &Path) -> HashMap<PathBuf, u64> {
  let mut total: HashMap<PathBuf, u64> = HashMap::new();
  for (dir, bytes) in direct {
    let mut p = dir.as_path();
    loop {
      *total.entry(p.to_path_buf()).or_insert(0) += bytes;
      match p.parent() {
        Some(q) if p != root && q.starts_with(root) => p = q,
        _ => break,
      }
    }
  }
  total
}

pub fn scan_usage(profile: PathBuf, progress: Progress) -> (Usage, Tree) {
  let windir = PathBuf::from(std::env::var_os("SystemDrive").map(|d| format!("{}\\", d.to_string_lossy())).unwrap_or_else(|| "C:\\".into()));
  let counter = AtomicU64::new(0);
  let size_of = |root: &Path| -> u64 { walk(root, &counter, &progress).values().sum() };

  progress.set("Measuring installed apps");
  let apps = size_of(&windir.join("Program Files")) + size_of(&windir.join("Program Files (x86)"));
  progress.set("Measuring shared app data");
  let shared = size_of(&windir.join("ProgramData"));
  progress.set("Measuring your folders");
  let direct = walk(&profile, &counter, &progress);
  let total = fold(&direct, &profile);
  let profile_total = total.get(&profile).copied().unwrap_or(0);
  let appdata = total.get(&profile.join("AppData")).copied().unwrap_or(0);

  let mut children: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
  for dir in total.keys() {
    if let Some(parent) = dir.parent().filter(|_| dir != &profile) {
      children.entry(parent.to_path_buf()).or_default().push(dir.clone());
    }
  }

  let (disk_total, free) = space(&windir.to_string_lossy());
  let used = disk_total.saturating_sub(free);
  let yours = profile_total.saturating_sub(appdata);
  let counted = apps + shared + profile_total;
  let usage = Usage {
    total: disk_total,
    used,
    categories: vec![
      Category { id: "yours", name: "Your files", bytes: yours },
      Category { id: "appdata", name: "App data and caches", bytes: appdata },
      Category { id: "apps", name: "Installed apps", bytes: apps },
      Category { id: "shared", name: "Shared app data", bytes: shared },
      Category { id: "system", name: "Windows and system", bytes: used.saturating_sub(counted) },
    ],
    profile: profile.to_string_lossy().into_owned(),
    files: counter.load(Ordering::Relaxed),
  };
  (usage, Tree { total, direct, children })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
  pub name: String,
  pub path: String,
  pub bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
  pub path: String,
  pub bytes: u64,
  pub files_here: u64,
  pub children: Vec<FolderEntry>,
}

pub fn folder(tree: &Tree, path: &Path) -> Option<Folder> {
  let bytes = *tree.total.get(path)?;
  let mut children: Vec<FolderEntry> = tree
    .children
    .get(path)
    .map(|kids| {
      kids.iter()
        .map(|k| FolderEntry {
          name: k.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
          path: k.to_string_lossy().into_owned(),
          bytes: tree.total.get(k).copied().unwrap_or(0),
        })
        .collect()
    })
    .unwrap_or_default();
  children.sort_by(|a, b| b.bytes.cmp(&a.bytes));
  children.truncate(40);
  Some(Folder { path: path.to_string_lossy().into_owned(), bytes, files_here: tree.direct.get(path).copied().unwrap_or(0), children })
}

// ---------- Deleting from the folder list ----------

impl Tree {
  /// Forgets a deleted folder and takes its size off every folder above it, so the list updates without a rescan.
  pub fn remove(&mut self, path: &Path) {
    let Some(bytes) = self.total.get(path).copied() else { return };
    self.total.retain(|p, _| !p.starts_with(path));
    self.direct.retain(|p, _| !p.starts_with(path));
    self.children.retain(|p, _| !p.starts_with(path));
    if let Some(kids) = path.parent().and_then(|p| self.children.get_mut(p)) {
      kids.retain(|c| c != path);
    }
    let mut p = path.parent();
    while let Some(q) = p {
      match self.total.get_mut(q) {
        Some(t) => *t = t.saturating_sub(bytes),
        None => break,
      }
      p = q.parent();
    }
  }
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contents {
  pub bytes: u64,
  pub files: u64,
  pub folders: u64,
}

const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;

/// The deletion gate for the Storage page's folder list: a folder inside the user folder, below its main folders, not in
/// app data or OneDrive, and neither inside nor holding a code project, Salon Shaila work, a Keep-list file or one of the
/// user's exclusions. Walks the whole folder, so it also returns what would be deleted.
pub fn check_folder(path: &Path, profile: &Path, rules: &crate::finder::Rules) -> Result<Contents, String> {
  use crate::finder::{is_project, CLIENT_WORK, KEEP};
  let rel = path.strip_prefix(profile).map_err(|_| "outside your user folder".to_string())?;
  let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
  if parts.iter().any(|p| p == "..") {
    return Err("not a plain folder path".into());
  }
  if parts.len() < 2 {
    return Err("one of your account's main folders; open it and delete inside".into());
  }
  if parts[0].eq_ignore_ascii_case("AppData") {
    return Err("app data; use Old caches for that".into());
  }
  if parts[0].to_lowercase().starts_with("onedrive") {
    return Err("synced with OneDrive, so deleting it would delete it online too".into());
  }
  let md = std::fs::symlink_metadata(path).map_err(|_| "no longer exists".to_string())?;
  if !md.is_dir() || md.file_type().is_symlink() {
    return Err("not a regular folder".into());
  }
  if md.file_attributes() & HIDDEN_OR_SYSTEM != 0 {
    return Err("a hidden or system folder".into());
  }
  if rules.excluded(path) {
    return Err("on your exclusions list".into());
  }
  let mut d = Some(path);
  while let Some(dir) = d.filter(|d| *d != profile) {
    if is_project(dir) {
      return Err(if dir == path { "a code project".into() } else { "inside a code project".into() });
    }
    if CLIENT_WORK.is_match(&dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) {
      return Err("Salon Shaila work".into());
    }
    d = dir.parent();
  }
  fn walk(dir: &Path, rules: &crate::finder::Rules, c: &mut Contents) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
      let name = entry.file_name().to_string_lossy().into_owned();
      let p = entry.path();
      if CLIENT_WORK.is_match(&name) {
        return Err(format!("holds Salon Shaila work ({name})"));
      }
      if rules.excluded(&p) {
        return Err(format!("holds something on your exclusions list ({name})"));
      }
      let Ok(ft) = entry.file_type() else { continue };
      if ft.is_dir() && !ft.is_symlink() {
        if is_project(&p) {
          return Err(format!("holds a code project ({name})"));
        }
        c.folders += 1;
        walk(&p, rules, c)?;
      } else if ft.is_file() {
        if KEEP.is_match(&name) {
          return Err(format!("holds a file on your Keep list ({name})"));
        }
        c.files += 1;
        c.bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
      }
    }
    Ok(())
  }
  let mut c = Contents { bytes: 0, files: 0, folders: 0 };
  walk(path, rules, &mut c)?;
  Ok(c)
}

/// The hibernation file's size (0 when hibernation is off).
pub fn hiberfil_bytes() -> u64 {
  let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
  std::fs::read_dir(format!("{drive}\\"))
    .into_iter()
    .flatten()
    .flatten()
    .find(|e| e.file_name().eq_ignore_ascii_case("hiberfil.sys"))
    .and_then(|e| e.metadata().ok())
    .map(|m| m.len())
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::finder::Rules;
  use std::fs;

  #[test]
  fn folder_gate() {
    let profile = std::env::temp_dir().join(format!("shimmy-storage-test-{}", std::process::id()));
    let mk = |rel: &str| {
      let p = profile.join(rel);
      fs::create_dir_all(&p).unwrap();
      p
    };
    let old = mk(r"Downloads\old stuff");
    fs::write(old.join("a.bin"), b"1234").unwrap();
    mk(r"Downloads\old stuff\inner");
    fs::write(old.join(r"inner\b.bin"), b"56").unwrap();
    let with_proj = mk(r"Desktop\work");
    mk(r"Desktop\work\site");
    fs::write(with_proj.join(r"site\package.json"), b"{}").unwrap();
    let shop = mk(r"Documents\clients\Salon Shaila");
    let keep = mk(r"Documents\books");
    fs::write(keep.join("Physics Student Book 2.pdf"), b"x").unwrap();
    let appdata = mk(r"AppData\Local\Thing");
    let r = Rules::default();

    assert_eq!(check_folder(&old, &profile, &r), Ok(Contents { bytes: 6, files: 2, folders: 1 }));
    assert_eq!(check_folder(&profile.join("Downloads"), &profile, &r).unwrap_err(), "one of your account's main folders; open it and delete inside");
    assert_eq!(check_folder(&with_proj, &profile, &r).unwrap_err(), "holds a code project (site)");
    assert_eq!(check_folder(&with_proj.join("site"), &profile, &r).unwrap_err(), "a code project");
    assert_eq!(check_folder(&profile.join(r"Documents\clients"), &profile, &r).unwrap_err(), "holds Salon Shaila work (Salon Shaila)");
    assert_eq!(check_folder(&shop, &profile, &r).unwrap_err(), "Salon Shaila work");
    assert_eq!(check_folder(&keep, &profile, &r).unwrap_err(), "holds a file on your Keep list (Physics Student Book 2.pdf)");
    assert_eq!(check_folder(&appdata, &profile, &r).unwrap_err(), "app data; use Old caches for that");
    assert_eq!(check_folder(&old, &profile, &Rules::new(500, &["OLD STUFF".into()])).unwrap_err(), "on your exclusions list");
    assert!(check_folder(Path::new(r"C:\Windows\System32"), &profile, &r).is_err());

    let mut t = Tree::default();
    let dl = profile.join("Downloads");
    for (p, b) in [(&profile, 10u64), (&dl, 8), (&old, 6)] {
      t.total.insert(p.clone(), b);
    }
    t.children.insert(dl.clone(), vec![old.clone()]);
    t.remove(&old);
    assert_eq!((t.total[&profile], t.total[&dl]), (4, 2));
    assert!(t.children[&dl].is_empty());
    let _ = fs::remove_dir_all(&profile);
  }
}
