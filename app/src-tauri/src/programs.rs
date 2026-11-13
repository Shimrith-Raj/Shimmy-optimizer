//! Programs page: installed apps by size, each removed with its own uninstaller (A9), and app updates through winget
//! (A8). Read only until the user confirms one action. The page only ever names a registry key or a winget id that
//! Shimmy listed itself; the command that runs is read here, never taken from the page.
use regex::Regex;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::LazyLock;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
use winreg::RegKey;

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

/// The three places Windows lists installed apps: (id prefix, hive, view flag).
fn roots() -> [(&'static str, RegKey, u32); 3] {
  [
    ("HKLM", RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_64KEY),
    ("HKLM32", RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_32KEY),
    ("HKCU", RegKey::predef(HKEY_CURRENT_USER), 0),
  ]
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
  /// "HKLM\<subkey>", "HKLM32\<subkey>" or "HKCU\<subkey>"; what the page sends back to uninstall.
  pub key: String,
  pub name: String,
  pub publisher: String,
  pub version: String,
  /// The installer's own estimate, which can be missing or out of date.
  pub bytes: Option<u64>,
  /// yyyy-mm-dd.
  pub installed: Option<String>,
  pub for_all_users: bool,
}

/// Install dates come as 20261028 or, from some installers, 5/13/2026.
pub fn parse_date(s: &str) -> Option<String> {
  let s = s.trim();
  if s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()) {
    return Some(format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..]));
  }
  let p: Vec<u32> = s.split('/').map(|x| x.parse().ok()).collect::<Option<_>>()?;
  match p[..] {
    [m, d, y] if (1..=12).contains(&m) && (1..=31).contains(&d) && y > 1990 => Some(format!("{y:04}-{m:02}-{d:02}")),
    _ => None,
  }
}

fn listed(k: &RegKey) -> Option<(String, String)> {
  let name: String = k.get_value("DisplayName").ok()?;
  let uninstall: String = k.get_value("UninstallString").ok()?;
  // Parts of other apps, updates and hidden system components are not offered.
  let system: u32 = k.get_value("SystemComponent").unwrap_or(0);
  let parent: Option<String> = k.get_value("ParentKeyName").ok();
  let kind: String = k.get_value("ReleaseType").unwrap_or_default();
  (system != 1 && parent.is_none() && !kind.contains("Update") && kind != "Hotfix" && !name.trim().is_empty() && !uninstall.trim().is_empty())
    .then(|| (name.trim().to_string(), uninstall))
}

pub fn list() -> Vec<Installed> {
  let mut out: Vec<Installed> = Vec::new();
  for (prefix, hive, view) in roots() {
    let Ok(root) = hive.open_subkey_with_flags(UNINSTALL, KEY_READ | view) else { continue };
    for sub in root.enum_keys().flatten() {
      let Ok(k) = root.open_subkey_with_flags(&sub, KEY_READ | view) else { continue };
      let Some((name, _)) = listed(&k) else { continue };
      // The same app can be listed in both views; keep the first.
      if out.iter().any(|i| i.name == name) {
        continue;
      }
      let kb: Option<u32> = k.get_value("EstimatedSize").ok();
      out.push(Installed {
        key: format!("{prefix}\\{sub}"),
        name,
        publisher: k.get_value::<String, _>("Publisher").unwrap_or_default().trim().to_string(),
        version: k.get_value::<String, _>("DisplayVersion").unwrap_or_default().trim().to_string(),
        bytes: kb.filter(|&k| k > 0).map(|k| k as u64 * 1024),
        installed: k.get_value::<String, _>("InstallDate").ok().as_deref().and_then(parse_date),
        for_all_users: prefix != "HKCU",
      });
    }
  }
  out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
  out
}

/// Splits an uninstall command into the program and its arguments: a quoted path, or an unquoted one up to ".exe".
pub fn split_command(s: &str) -> Option<(String, String)> {
  let s = s.trim();
  if let Some(rest) = s.strip_prefix('"') {
    let end = rest.find('"')?;
    return Some((rest[..end].to_string(), rest[end + 1..].trim().to_string()));
  }
  let lower = s.to_ascii_lowercase();
  match lower.find(".exe") {
    Some(i) => Some((s[..i + 4].to_string(), s[i + 4..].trim().to_string())),
    // Windows Installer is often listed with no extension: `msiexec /x{code}`.
    None => {
      let (first, rest) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
      first.eq_ignore_ascii_case("msiexec").then(|| ("msiexec.exe".to_string(), rest.trim().to_string()))
    }
  }
}

/// Windows Installer apps often list `msiexec /I{code}`, which opens the repair screen; /X is the uninstall.
pub fn msi_uninstall(exe: &str, args: &str) -> String {
  let is_msi = exe.to_ascii_lowercase().trim_end_matches(".exe").ends_with("msiexec");
  if !is_msi {
    return args.to_string();
  }
  ["/I{", "/i{", "/I {", "/i {"].iter().fold(args.to_string(), |a, from| a.replacen(from, "/X{", 1))
}

/// The full path of an uninstaller when Shimmy may start it. A bare name is Windows' own (msiexec) and is looked for only
/// in System32, never along PATH. None when its file or folder is not an administrator's.
pub fn trusted_uninstaller(exe: &str) -> Option<PathBuf> {
  let p = PathBuf::from(exe);
  let full = if p.is_absolute() { p } else { crate::clean::known("SystemRoot")?.join("System32").join(p) };
  crate::update::trusted_program(&full).then_some(full)
}

/// Apps installed just for this user are listed under HKCU, which any program the user runs can write. Shimmy runs as
/// administrator, so it never starts their uninstall command itself: the page sends those to Windows' Installed apps.
pub fn per_user(key: &str) -> bool {
  key.starts_with("HKCU\\")
}

/// The app's name and its uninstall command as listed, read again from the registry for this key.
fn entry(key: &str) -> Result<(String, String), String> {
  let (prefix, sub) = key.split_once('\\').ok_or("Shimmy does not know that app.")?;
  if sub.is_empty() || sub.contains('\\') {
    return Err("Shimmy does not know that app.".into());
  }
  let (_, hive, view) = roots().into_iter().find(|(p, _, _)| *p == prefix).ok_or("Shimmy does not know that app.")?;
  let k = hive.open_subkey_with_flags(format!("{UNINSTALL}\\{sub}"), KEY_READ | view).map_err(|_| "That app is no longer installed.")?;
  listed(&k).ok_or_else(|| "Windows does not offer an uninstaller for that app.".to_string())
}

/// The name of an app Shimmy listed, for a key the page sent back.
pub fn name_of(key: &str) -> Result<String, String> {
  entry(key).map(|e| e.0)
}

/// The app's name and its uninstall command (program, arguments). Only for apps installed for all users (HKLM, which
/// only administrators can write).
pub fn uninstall_command(key: &str) -> Result<(String, String, String), String> {
  if per_user(key) {
    return Err("That app was installed just for you, so Shimmy does not start its uninstaller as administrator. Use Windows' Installed apps page.".into());
  }
  let (name, cmd) = entry(key)?;
  if name.eq_ignore_ascii_case("Shimmy Optimizer") {
    return Err("Shimmy cannot uninstall itself. Use Windows' Installed apps page.".into());
  }
  let (exe, args) = split_command(&cmd).ok_or("Its uninstall command could not be read.")?;
  let args = msi_uninstall(&exe, &args);
  Ok((name, exe, args))
}

// ---------- winget ----------

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Upgrade {
  pub name: String,
  pub id: String,
  pub version: String,
  pub available: String,
}

static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$").unwrap());

/// A winget package id as winget prints it, such as Git.Git. Anything else is never passed to winget.
pub fn valid_id(id: &str) -> bool {
  ID.is_match(id)
}

/// Reads `winget upgrade`'s table. Columns are found from where each header word starts, so it does not depend on the
/// header's language. Rows end at the first blank line (a second table, of apps that need explicit targeting, follows).
/// A cut-off id (ending in …) cannot be targeted, so that row is left out.
pub fn parse_upgrades(text: &str) -> Vec<Upgrade> {
  // winget redraws a spinner with carriage returns; only what follows the last one on a line is real.
  let lines: Vec<Vec<char>> = text.lines().map(|l| l.rsplit('\r').next().unwrap_or("").chars().collect()).collect();
  let Some(dash) = lines.iter().position(|l| l.len() > 20 && l.iter().all(|&c| c == '-')) else { return Vec::new() };
  let Some(head) = dash.checked_sub(1).map(|i| &lines[i]) else { return Vec::new() };
  let starts: Vec<usize> = (0..head.len()).filter(|&i| head[i] != ' ' && (i == 0 || head[i - 1] == ' ')).collect();
  if starts.len() < 4 {
    return Vec::new();
  }
  let cell = |l: &[char], c: usize| -> String {
    let from = starts[c].min(l.len());
    let to = starts.get(c + 1).copied().unwrap_or(l.len()).min(l.len());
    l[from..to].iter().collect::<String>().trim().to_string()
  };
  lines[dash + 1..]
    .iter()
    .take_while(|l| l.iter().any(|c| !c.is_whitespace()))
    .filter(|l| l.len() > starts[3])
    .map(|l| Upgrade { name: cell(l, 0), id: cell(l, 1), version: cell(l, 2), available: cell(l, 3) })
    .filter(|u| valid_id(&u.id) && !u.available.is_empty())
    .collect()
}

/// Whether `loc` is winget's own package folder: under Program Files\WindowsApps (which only Windows can write) and
/// named for Microsoft's App Installer package.
pub fn winget_dir_ok(loc: &str, program_files: &str) -> bool {
  let (loc, base) = (loc.trim().to_ascii_lowercase(), format!("{}\\windowsapps\\", program_files.trim_end_matches('\\').to_ascii_lowercase()));
  let Some(name) = loc.strip_prefix(&base) else { return false };
  !name.contains('\\') &&!name.contains("..") && name.starts_with("microsoft.desktopappinstaller_") && name.ends_with("__8wekyb3d8bbwe")
}

/// winget.exe in its package folder, found through Windows' package list. Never `winget.exe` by name: that is found
/// through the user's PATH, and its usual folder (%LOCALAPPDATA%\Microsoft\WindowsApps) is one the user can write to,
/// so a planted copy would run as administrator. None when App Installer is not on this PC.
fn winget_exe() -> Option<std::path::PathBuf> {
  let pf = crate::clean::known("ProgramFiles")?;
  let out = crate::clean::powershell(&["Appx"], "(Get-AppxPackage -Name Microsoft.DesktopAppInstaller).InstallLocation").ok()?;
  let text = String::from_utf8_lossy(&out.stdout);
  let loc = text.lines().map(str::trim).find(|l| winget_dir_ok(l, &pf.to_string_lossy()))?;
  Some(std::path::Path::new(loc).join("winget.exe")).filter(|p| p.is_file())
}

/// How long winget may take: listing updates waits on the network; an update also runs the app's own installer.
const CHECK_LIMIT: std::time::Duration = std::time::Duration::from_secs(3 * 60);
const UPDATE_LIMIT: std::time::Duration = std::time::Duration::from_secs(20 * 60);

const AGREE: [&str; 3] = ["--accept-source-agreements", "--disable-interactivity", "--nowarn"];

/// Apps with an update, or None when winget is not on this PC.
pub fn upgrades() -> Result<Option<Vec<Upgrade>>, String> {
  let Some(exe) = winget_exe() else { return Ok(None) };
  let out = match crate::clean::hidden_for(&exe.to_string_lossy(), &[&["upgrade"][..], &AGREE[..]].concat(), CHECK_LIMIT) {
    Ok(o) => o,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return Err("winget did not answer within 3 minutes. Check your connection and try again.".into()),
    Err(e) => return Err(format!("Could not start winget: {e}")),
  };
  Ok(Some(parse_upgrades(&String::from_utf8_lossy(&out.stdout))))
}

/// Updates one app with winget. The installer may show its own window.
pub fn upgrade(id: &str) -> Result<(), String> {
  if !valid_id(id) {
    return Err("That is not a winget package id.".into());
  }
  let args = [&["upgrade", "--id", id, "--exact", "--accept-package-agreements"][..], &AGREE[..]].concat();
  let exe = winget_exe().ok_or("winget, Windows' app installer, is not on this PC.")?;
  let out = crate::clean::hidden_for(&exe.to_string_lossy(), &args, UPDATE_LIMIT).map_err(|e| match e.kind() {
    std::io::ErrorKind::TimedOut => "The update took longer than 20 minutes, so Shimmy stopped waiting for it.".to_string(),
    _ => format!("Could not start winget: {e}"),
  })?;
  if out.status.success() {
    return Ok(());
  }
  let text = String::from_utf8_lossy(&out.stdout);
  let last = text.lines().map(|l| l.rsplit('\r').next().unwrap_or("").trim()).filter(|l| !l.is_empty() && !l.chars().all(|c| "-\\|/ █▒".contains(c))).last();
  Err(last.unwrap_or("winget could not update it.").to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reads_winget_upgrade() {
    let text = "\r   - \r   \\ \r\
Name                                 Id                    Version   Available  Source\n\
-------------------------------------------------------------------------------------\n\
Git                                  Git.Git               2.55.0.3  2.56.0.3   winget\n\
Microsoft Visual C++ 2015-2022 Re…   Microsoft.VCRedist.2… 14.44     14.51      winget\n\
Node.js                              OpenJS.NodeJS.LTS     24.18.0   26.13.0    winget\n\
3 upgrades available.\n\
\n\
The following packages have an upgrade available, but require explicit targeting for upgrade:\n";
    let u = parse_upgrades(text);
    assert_eq!(u.len(), 2, "the cut-off id is left out, and the summary line is not a row");
    assert_eq!(u[0], Upgrade { name: "Git".into(), id: "Git.Git".into(), version: "2.55.0.3".into(), available: "2.56.0.3".into() });
    assert_eq!(u[1].id, "OpenJS.NodeJS.LTS");
    assert!(parse_upgrades("No installed package found matching input criteria.").is_empty());
    assert!(valid_id("Microsoft.VCRedist.2015+.x64"));
    assert!(!valid_id("Git.Git & calc") && !valid_id("--source") && !valid_id(""));
  }

  #[test]
  fn uninstall_commands() {
    assert_eq!(split_command(r#""C:\Program Files\cursor\unins000.exe" /SILENT"#), Some((r"C:\Program Files\cursor\unins000.exe".into(), "/SILENT".into())));
    assert_eq!(split_command(r"C:\Program Files\App\uninstall.exe --remove"), Some((r"C:\Program Files\App\uninstall.exe".into(), "--remove".into())));
    assert_eq!(split_command("MsiExec.exe /I{1234-ABCD}"), Some(("MsiExec.exe".into(), "/I{1234-ABCD}".into())));
    assert_eq!(msi_uninstall("MsiExec.exe", "/I{1234-ABCD}"), "/X{1234-ABCD}");
    assert_eq!(msi_uninstall(r"C:\App\unins000.exe", "/I{x}"), "/I{x}", "only for Windows Installer");
    assert_eq!(split_command("no program here"), None);
    assert_eq!(split_command("msiexec /x{1234-ABCD}"), Some(("msiexec.exe".into(), "/x{1234-ABCD}".into())), "Windows Installer with no extension");
    assert_eq!(msi_uninstall("msiexec.exe", "/I {1234-ABCD}"), "/X{1234-ABCD}", "with a space too");
    assert!(trusted_uninstaller("msiexec.exe").is_some_and(|p| p.is_absolute()), "a bare name is Windows' own, from System32");
    let planted = std::env::temp_dir().join(format!("shimmy-uninstaller-{}.exe", std::process::id()));
    std::fs::write(&planted, b"x").unwrap();
    assert!(trusted_uninstaller(&planted.to_string_lossy()).is_none(), "a file a standard user owns, in a folder they can write");
    let _ = std::fs::remove_file(&planted);
  }

  #[test]
  fn only_trusted_uninstallers_and_winget() {
    assert!(per_user(r"HKCU\{abc}") && !per_user(r"HKLM\{abc}") && !per_user(r"HKLM32\Foo"));
    assert!(uninstall_command(r"HKCU\Anything").unwrap_err().contains("just for you"));
    let pf = r"C:\Program Files";
    assert!(winget_dir_ok(r"C:\Program Files\WindowsApps\Microsoft.DesktopAppInstaller_1.29.379.0_x64__8wekyb3d8bbwe", pf));
    assert!(!winget_dir_ok(r"C:\Users\me\AppData\Local\Microsoft\WindowsApps", pf), "the user-writable alias folder");
    assert!(!winget_dir_ok(r"C:\Program Files\WindowsApps\Evil_1.0_x64__abc", pf));
    assert!(!winget_dir_ok(r"C:\Program Files\WindowsApps\Microsoft.DesktopAppInstaller_1_x64__8wekyb3d8bbwe\..\x", pf));
  }

  #[test]
  fn install_dates() {
    assert_eq!(parse_date("20261028").as_deref(), Some("2026-10-28"));
    assert_eq!(parse_date("5/13/2026").as_deref(), Some("2026-05-13"));
    assert_eq!(parse_date("yesterday"), None);
  }
}
