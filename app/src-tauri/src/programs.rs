//! Programs page: installed apps by size, each removed with its own uninstaller (A9), and app updates through winget
//! (A8). Read only until the user confirms one action. The page only ever names a registry key or a winget id that
//! Shimmy listed itself; the command that runs is read here, never taken from the page.
use regex::Regex;
use serde::Serialize;
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
  let end = s.to_ascii_lowercase().find(".exe")? + 4;
  Some((s[..end].to_string(), s[end..].trim().to_string()))
}

/// Windows Installer apps often list `msiexec /I{code}`, which opens the repair screen; /X is the uninstall.
pub fn msi_uninstall(exe: &str, args: &str) -> String {
  let is_msi = exe.to_ascii_lowercase().trim_end_matches(".exe").ends_with("msiexec");
  if is_msi { args.replacen("/I{", "/X{", 1).replacen("/i{", "/X{", 1) } else { args.to_string() }
}

/// The app's name and its uninstall command (program, arguments), read again from the registry for this key.
pub fn uninstall_command(key: &str) -> Result<(String, String, String), String> {
  let (prefix, sub) = key.split_once('\\').ok_or("Shimmy does not know that app.")?;
  if sub.is_empty() || sub.contains('\\') {
    return Err("Shimmy does not know that app.".into());
  }
  let (_, hive, view) = roots().into_iter().find(|(p, _, _)| *p == prefix).ok_or("Shimmy does not know that app.")?;
  let k = hive.open_subkey_with_flags(format!("{UNINSTALL}\\{sub}"), KEY_READ | view).map_err(|_| "That app is no longer installed.")?;
  let (name, cmd) = listed(&k).ok_or("Windows does not offer an uninstaller for that app.")?;
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

const AGREE: [&str; 3] = ["--accept-source-agreements", "--disable-interactivity", "--nowarn"];

/// Apps with an update, or None when winget is not on this PC.
pub fn upgrades() -> Result<Option<Vec<Upgrade>>, String> {
  let out = match crate::clean::hidden("winget.exe", &[&["upgrade"][..], &AGREE[..]].concat()) {
    Ok(o) => o,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
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
  let out = crate::clean::hidden("winget.exe", &args).map_err(|e| format!("Could not start winget: {e}"))?;
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
  }

  #[test]
  fn install_dates() {
    assert_eq!(parse_date("20261028").as_deref(), Some("2026-10-28"));
    assert_eq!(parse_date("5/13/2026").as_deref(), Some("2026-05-13"));
    assert_eq!(parse_date("yesterday"), None);
  }
}
