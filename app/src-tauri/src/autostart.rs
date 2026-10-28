//! Start with Windows through a Task Scheduler task with highest privileges, so the elevated app starts at sign-in
//! without a UAC prompt (tested in Phase 0). Default task settings refuse to start on battery, so both battery
//! settings are turned off here.
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const TASK: &str = "Shimmy Optimizer";
/// Passed by the task so Shimmy starts in the tray instead of opening its window.
pub const TRAY_ARG: &str = "--tray";

fn schtasks(args: &[&str]) -> Result<String, String> {
  let out = Command::new("schtasks.exe").args(args).stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
  if out.status.success() {
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
  } else {
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
  }
}

pub fn enabled() -> bool {
  schtasks(&["/Query", "/TN", TASK]).is_ok()
}

fn xml_escape(s: &str) -> String {
  s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn task_xml(exe: &Path, user: &str) -> String {
  format!(
    r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts Shimmy Optimizer in the tray at sign-in.</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId><Delay>PT10S</Delay></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>"{exe}"</Command><Arguments>{arg}</Arguments></Exec></Actions>
</Task>"#,
    user = xml_escape(user),
    exe = xml_escape(&exe.to_string_lossy()),
    arg = TRAY_ARG,
  )
}

pub fn set(on: bool) -> Result<(), String> {
  if !on {
    return if enabled() { schtasks(&["/Delete", "/TN", TASK, "/F"]).map(|_| ()) } else { Ok(()) };
  }
  let exe = std::env::current_exe().map_err(|e| e.to_string())?;
  let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
    (Ok(d), Ok(u)) => format!("{d}\\{u}"),
    (_, Ok(u)) => u,
    _ => return Err("Could not tell which Windows user is signed in.".into()),
  };
  // schtasks reads task XML as UTF-16.
  let path = std::env::temp_dir().join(format!("shimmy-task-{}.xml", std::process::id()));
  let mut bytes = vec![0xFF, 0xFE];
  bytes.extend(task_xml(&exe, &user).encode_utf16().flat_map(|u| u.to_le_bytes()));
  std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
  let r = schtasks(&["/Create", "/TN", TASK, "/XML", &path.to_string_lossy(), "/F"]);
  let _ = std::fs::remove_file(&path);
  r.map(|_| ())
}

#[cfg(test)]
mod tests {
  #[test]
  fn task_xml_runs_on_battery_elevated() {
    let x = super::task_xml(std::path::Path::new(r"C:\Program Files\Shimmy Optimizer\shimmy-optimizer.exe"), r"PC\Studio & Co");
    assert!(x.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
    assert!(x.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
    assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
    assert!(x.contains(r"<UserId>PC\Studio &amp; Co</UserId>"));
    assert!(x.contains("<Arguments>--tray</Arguments>"));
  }
}
