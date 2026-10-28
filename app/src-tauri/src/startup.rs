//! Startup apps, read and switched through the StartupApproved flags (the same mechanism as Task Manager).
//! Shimmy runs elevated, so all-users entries can be switched too.
use serde::Serialize;
use std::path::PathBuf;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_BINARY};
use winreg::{RegKey, RegValue};

const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
  pub name: String,
  pub command: String,
  pub source: String,
  pub scope: String,
  /// Which StartupApproved subkey holds its flag: Run, Run32 or StartupFolder.
  pub sub: String,
  pub enabled: bool,
  pub can_toggle: bool,
}

fn hive(scope: &str) -> RegKey {
  RegKey::predef(if scope == "Machine" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER })
}

/// First byte odd means disabled; a missing flag means enabled.
fn approved(scope: &str, sub: &str, name: &str) -> bool {
  hive(scope)
    .open_subkey(format!(r"{APPROVED}\{sub}"))
    .and_then(|k| k.get_raw_value(name))
    .map(|v| v.bytes.first().map(|b| b & 1 == 0).unwrap_or(true))
    .unwrap_or(true)
}

pub fn list() -> Vec<Entry> {
  let mut out = Vec::new();
  for (scope, key, sub) in [("User", RUN, "Run"), ("Machine", RUN, "Run"), ("Machine", RUN32, "Run32")] {
    let Ok(k) = hive(scope).open_subkey(key) else { continue };
    for (name, _) in k.enum_values().flatten() {
      if name.is_empty() {
        continue;
      }
      out.push(Entry {
        command: k.get_value::<String, _>(&name).unwrap_or_default(),
        enabled: approved(scope, sub, &name),
        name,
        source: "Run".into(),
        scope: scope.into(),
        sub: sub.into(),
        can_toggle: true,
      });
    }
  }
  if let Some(appdata) = std::env::var_os("APPDATA") {
    let folder = PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
    for f in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
      let name = f.file_name().to_string_lossy().into_owned();
      if name.eq_ignore_ascii_case("desktop.ini") || !f.path().is_file() {
        continue;
      }
      out.push(Entry {
        enabled: approved("User", "StartupFolder", &name),
        command: f.path().to_string_lossy().into_owned(),
        name,
        source: "Startup folder".into(),
        scope: "User".into(),
        sub: "StartupFolder".into(),
        can_toggle: true,
      });
    }
  }
  out
}

pub fn toggle(name: &str, sub: &str, scope: &str, enable: bool) -> Result<(), String> {
  if !["Run", "Run32", "StartupFolder"].contains(&sub) {
    return Err("Unknown startup source.".into());
  }
  let (key, _) = hive(scope).create_subkey(format!(r"{APPROVED}\{sub}")).map_err(|e| e.to_string())?;
  let mut bytes = vec![0u8; 12];
  bytes[0] = if enable { 2 } else { 3 };
  key.set_raw_value(name, &RegValue { bytes: bytes.into(), vtype: REG_BINARY }).map_err(|e| e.to_string())
}
