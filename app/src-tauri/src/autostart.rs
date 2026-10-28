//! Start with Windows through a Task Scheduler task with highest privileges, so the elevated app starts at sign-in
//! without a UAC prompt (tested in Phase 0). Default task settings refuse to start on battery, so both battery
//! settings are turned off here.
use std::path::Path;
use windows::core::BSTR;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::TaskScheduler::{ITaskFolder, ITaskService, TaskScheduler, TASK_CREATE_OR_UPDATE, TASK_LOGON_INTERACTIVE_TOKEN};
use windows::Win32::System::Variant::VARIANT;

pub const TASK: &str = "Shimmy Optimizer";
/// Passed by the task so Shimmy starts in the tray instead of opening its window.
pub const TRAY_ARG: &str = "--tray";

/// Balances CoInitializeEx. When the thread already has COM in another mode, that is left as it is.
struct Com(bool);

impl Drop for Com {
  fn drop(&mut self) {
    if self.0 {
      unsafe { CoUninitialize() };
    }
  }
}

/// Runs `f` on the root folder of the local Task Scheduler. The COM objects are released before COM is torn down.
fn with_root<T>(f: impl FnOnce(&ITaskFolder) -> windows::core::Result<T>) -> Result<T, String> {
  let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
  if hr.is_err() && hr != RPC_E_CHANGED_MODE {
    return Err(windows::core::Error::from(hr).message());
  }
  let _com = Com(hr.is_ok());
  let run = || -> windows::core::Result<T> {
    unsafe {
      let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
      let none = VARIANT::default();
      service.Connect(&none, &none, &none, &none)?;
      f(&service.GetFolder(&BSTR::from("\\"))?)
    }
  };
  run().map_err(|e| e.message())
}

pub fn enabled() -> bool {
  with_root(|root| unsafe { root.GetTask(&BSTR::from(TASK)).map(|_| ()) }).is_ok()
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
    return if enabled() { with_root(|root| unsafe { root.DeleteTask(&BSTR::from(TASK), 0) }) } else { Ok(()) };
  }
  let exe = std::env::current_exe().map_err(|e| e.to_string())?;
  let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
    (Ok(d), Ok(u)) => format!("{d}\\{u}"),
    (_, Ok(u)) => u,
    _ => return Err("Could not tell which Windows user is signed in.".into()),
  };
  let xml = BSTR::from(task_xml(&exe, &user));
  with_root(|root| unsafe {
    let none = VARIANT::default();
    root.RegisterTask(&BSTR::from(TASK), &xml, TASK_CREATE_OR_UPDATE.0, &none, &none, TASK_LOGON_INTERACTIVE_TOKEN, &none).map(|_| ())
  })
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
