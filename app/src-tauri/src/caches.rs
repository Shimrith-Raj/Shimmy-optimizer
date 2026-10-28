//! Old caches on the Storage page: known cache folders of apps on this laptop. Only files untouched for `age_days` are
//! offered, a cache is skipped while its app runs, and everything goes to the Recycle Bin (PLAN.md, Phase 8).
use crate::jobs::Progress;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct CachePrefs {
  /// Files untouched for this many days count as old.
  pub age_days: u32,
}

impl Default for CachePrefs {
  fn default() -> Self {
    CachePrefs { age_days: 30 }
  }
}

struct Def {
  id: &'static str,
  family: &'static str,
  title: &'static str,
  why: &'static str,
  default_on: bool,
  /// Lower-case process names; while one runs, the cache is skipped.
  procs: &'static [&'static str],
  /// (base environment variable, relative folder)
  dirs: &'static [(&'static str, &'static str)],
}

const DEFS: [Def; 10] = [
  Def { id: "capcut", family: "Editing apps", title: "CapCut cache", why: "Previews and effects CapCut downloaded or rendered. It makes them again when a project needs them.", default_on: true,
    procs: &["capcut.exe"], dirs: &[("LOCALAPPDATA", r"CapCut\User Data\Cache")] },
  Def { id: "premiere", family: "Editing apps", title: "Premiere and Media Encoder media cache", why: "Conformed audio and peak files. Premiere rebuilds them when a project opens.", default_on: true,
    procs: &["adobe premiere pro.exe", "adobe media encoder.exe", "afterfx.exe"],
    dirs: &[("APPDATA", r"Adobe\Common\Media Cache Files"), ("APPDATA", r"Adobe\Common\Peak Files"), ("APPDATA", r"Adobe\Common\Media Cache")] },
  Def { id: "telegram", family: "Apps", title: "Telegram media cache", why: "Photos and videos Telegram downloaded to show chats. Your messages stay in the cloud.", default_on: true,
    procs: &["telegram.exe"], dirs: &[("APPDATA", r"Telegram Desktop\tdata\user_data")] },
  Def { id: "spotify", family: "Apps", title: "Spotify cache", why: "Songs and images Spotify stored to play faster. Downloads you chose for offline are kept by Spotify itself.", default_on: true,
    procs: &["spotify.exe"], dirs: &[("LOCALAPPDATA", r"Spotify\Data"), ("LOCALAPPDATA", r"Spotify\Browser\Cache")] },
  Def { id: "discord", family: "Apps", title: "Discord cache", why: "Cached images and code. Settings and logins are elsewhere and stay.", default_on: true,
    procs: &["discord.exe"], dirs: &[("APPDATA", r"discord\Cache"), ("APPDATA", r"discord\Code Cache"), ("APPDATA", r"discord\GPUCache")] },
  Def { id: "claude", family: "Apps", title: "Claude desktop cache", why: "Cached pages and code of the Claude app. Chats and settings stay.", default_on: true,
    procs: &["claude.exe"], dirs: &[("APPDATA", r"Claude\Cache"), ("APPDATA", r"Claude\Code Cache"), ("APPDATA", r"Claude\GPUCache")] },
  Def { id: "shaders", family: "Graphics", title: "Graphics shader caches", why: "Compiled shaders from Windows, NVIDIA and AMD. Games rebuild them, which can make the first minutes a little choppy.", default_on: true,
    procs: &[],
    dirs: &[("LOCALAPPDATA", "D3DSCache"), ("LOCALAPPDATA", r"NVIDIA\DXCache"), ("LOCALAPPDATA", r"NVIDIA\GLCache"), ("LOCALAPPDATA", r"AMD\DxCache"), ("LOCALAPPDATA", r"AMD\DxcCache")] },
  Def { id: "npm", family: "Developer", title: "npm cache", why: "Packages npm downloaded. They are downloaded again when a project needs them.", default_on: false,
    procs: &[], dirs: &[("LOCALAPPDATA", "npm-cache")] },
  Def { id: "pip", family: "Developer", title: "pip cache", why: "Python packages pip downloaded or built. They are fetched again when needed.", default_on: false,
    procs: &[], dirs: &[("LOCALAPPDATA", r"pip\Cache")] },
  Def { id: "cargo", family: "Developer", title: "Cargo registry cache", why: "Rust crates and their unpacked sources. Building Shimmy downloads them again.", default_on: false,
    procs: &[], dirs: &[("USERPROFILE", r".cargo\registry\cache"), ("USERPROFILE", r".cargo\registry\src")] },
];

pub fn is_cache(id: &str) -> bool {
  DEFS.iter().any(|d| d.id == id)
}

fn dirs(d: &Def) -> Vec<PathBuf> {
  d.dirs.iter().filter_map(|(var, rel)| std::env::var_os(var).map(|b| PathBuf::from(b).join(rel))).filter(|p| p.is_dir()).collect()
}

fn blocked(d: &Def, running: &HashSet<String>) -> Option<String> {
  d.procs.iter().any(|p| running.contains(*p)).then(|| format!("{} is open. Close it to clean its cache.", d.title.trim_end_matches(" cache").trim_end_matches(" media")))
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CacheScan {
  pub id: &'static str,
  pub family: &'static str,
  pub title: &'static str,
  pub why: &'static str,
  pub default_on: bool,
  pub total: u64,
  /// Bytes in files older than the age setting.
  pub old: u64,
  pub old_files: u64,
  pub blocked: Option<String>,
  pub folders: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CacheResult {
  pub id: &'static str,
  pub title: &'static str,
  pub recycled: u64,
  pub files: u64,
  pub note: Option<String>,
}

fn old_enough(md: &fs::Metadata, cutoff: SystemTime) -> bool {
  md.modified().map(|m| m < cutoff).unwrap_or(false)
}

/// What to recycle under `dir`: a folder whose every file is old goes as one item (much faster than file by file),
/// otherwise its old files and fully old subfolders. Returns (items, bytes, files, all old?).
fn plan(dir: &Path, cutoff: SystemTime) -> (Vec<PathBuf>, u64, u64, bool) {
  let (mut items, mut bytes, mut files, mut all_old, mut any) = (Vec::new(), 0u64, 0u64, true, false);
  let mut whole_dirs = Vec::new();
  for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
    let Ok(ft) = entry.file_type() else { continue };
    if ft.is_symlink() {
      all_old = false;
      continue;
    }
    any = true;
    if ft.is_dir() {
      let (sub, b, f, sub_all) = plan(&entry.path(), cutoff);
      bytes += b;
      files += f;
      if sub_all {
        whole_dirs.push(entry.path());
      } else {
        all_old = false;
        items.extend(sub);
      }
    } else if let Ok(md) = entry.metadata() {
      if old_enough(&md, cutoff) {
        items.push(entry.path());
        bytes += md.len();
        files += 1;
      } else {
        all_old = false;
      }
    }
  }
  if all_old && any {
    return (Vec::new(), bytes, files, true);
  }
  items.extend(whole_dirs);
  (items, bytes, files, false)
}

fn total_size(dir: &Path) -> u64 {
  fs::read_dir(dir)
    .into_iter()
    .flatten()
    .flatten()
    .map(|e| match e.file_type() {
      Ok(t) if t.is_dir() && !t.is_symlink() => total_size(&e.path()),
      Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
      _ => 0,
    })
    .sum()
}

fn cutoff(age_days: u32) -> SystemTime {
  SystemTime::now() - Duration::from_secs(age_days as u64 * 86_400)
}

pub fn scan(prefs: &CachePrefs, running: &HashSet<String>, progress: &Progress) -> Vec<CacheScan> {
  let cut = cutoff(prefs.age_days);
  DEFS.iter()
    .filter_map(|d| {
      let ds = dirs(d);
      if ds.is_empty() {
        return None;
      }
      progress.set(format!("Measuring the {}", d.title.to_lowercase()));
      let (mut total, mut old, mut old_files) = (0, 0, 0);
      for dir in &ds {
        total += total_size(dir);
        let (_, b, f, _) = plan(dir, cut);
        old += b;
        old_files += f;
      }
      Some(CacheScan {
        id: d.id, family: d.family, title: d.title, why: d.why, default_on: d.default_on, total, old, old_files,
        blocked: blocked(d, running), folders: ds.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
      })
    })
    .filter(|c| c.total > 0)
    .collect()
}

/// Moves the old files of the chosen caches to the Recycle Bin. The cache folders themselves stay.
pub fn run(ids: &[String], prefs: &CachePrefs, running: &HashSet<String>, progress: &Progress) -> Vec<CacheResult> {
  let cut = cutoff(prefs.age_days);
  let mut out = Vec::new();
  for d in DEFS.iter().filter(|d| ids.iter().any(|i| i == d.id)) {
    let r = |recycled, files, note| CacheResult { id: d.id, title: d.title, recycled, files, note };
    if let Some(why) = blocked(d, running) {
      out.push(r(0, 0, Some(why)));
      continue;
    }
    progress.set(format!("Moving the old {} to the Recycle Bin", d.title.to_lowercase()));
    let (mut recycled, mut files, mut failed) = (0, 0, 0);
    for dir in dirs(d) {
      let (mut items, b, f, all) = plan(&dir, cut);
      if all {
        // Everything is old: recycle what is inside, keep the cache folder itself.
        items = fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
      }
      if items.is_empty() {
        continue;
      }
      // One shell operation per batch keeps it fast; a failed batch is retried item by item.
      for chunk in items.chunks(500) {
        if trash::delete_all(chunk).is_err() {
          for p in chunk {
            if trash::delete(p).is_err() {
              failed += 1;
            }
          }
        }
      }
      recycled += b;
      files += f;
    }
    out.push(r(recycled, files, (failed > 0).then(|| format!("{failed} items were in use and left in place."))));
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn plans_whole_old_folders_as_one_item() {
    let base = std::env::temp_dir().join(format!("shimmy-cache-test-{}", std::process::id()));
    let old = base.join("old");
    let mixed = base.join("mixed");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&mixed).unwrap();
    for p in [old.join("a"), old.join("b"), mixed.join("c"), mixed.join("new")] {
      fs::write(&p, b"12345").unwrap();
    }
    let past = SystemTime::now() - Duration::from_secs(40 * 86_400);
    for p in [old.join("a"), old.join("b"), mixed.join("c")] {
      fs::File::options().write(true).open(&p).unwrap().set_modified(past).unwrap();
    }
    let (items, bytes, files, all) = plan(&base, cutoff(30));
    assert!(!all);
    assert_eq!((bytes, files), (15, 3));
    assert!(items.contains(&old), "fully old folder is one item");
    assert!(items.contains(&mixed.join("c")));
    assert!(!items.contains(&mixed.join("new")), "recent files stay");
    assert_eq!(items.len(), 2);
    let _ = fs::remove_dir_all(&base);
  }
}
