//! Background priority (A11): apps the user picks run at a lower priority, so what they are working on gets the
//! processor first. Below normal or Low through SetPriorityClass; Efficiency mode is Low plus Windows' EcoQoS power
//! throttling, the same as Task Manager's. Applied to each new process of that app as it starts (checked every 30 s),
//! and every process is put back the way it was on Exit. Nothing is suspended.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
  GetPriorityClass, OpenProcess, SetPriorityClass, SetProcessInformation, ProcessPowerThrottling, BELOW_NORMAL_PRIORITY_CLASS, IDLE_PRIORITY_CLASS,
  PROCESS_CREATION_FLAGS, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
  PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
  Below,
  Low,
  Eco,
}

impl Level {
  pub fn words(self) -> &'static str {
    match self {
      Level::Below => "Below normal",
      Level::Low => "Low",
      Level::Eco => "Efficiency mode",
    }
  }
}

/// "below", "low" and "eco" set a level; "normal" (None) removes it.
pub fn parse_level(s: &str) -> Result<Option<Level>, String> {
  match s {
    "below" => Ok(Some(Level::Below)),
    "low" => Ok(Some(Level::Low)),
    "eco" => Ok(Some(Level::Eco)),
    "normal" => Ok(None),
    _ => Err("The priority must be normal, below, low or eco.".into()),
  }
}

/// Levels by lower-case exe name, as kept in settings.json. Unknown entries are dropped.
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, Level>, D::Error> {
  let raw = BTreeMap::<String, String>::deserialize(d)?;
  Ok(raw.into_iter().filter_map(|(k, v)| Some((crate::procs::exe_key(&k).ok()?, parse_level(&v).ok()??))).take(100).collect())
}

/// What Shimmy changed, by process id: the exe, the priority class it had before, and the level set.
pub type Applied = HashMap<u32, (String, u32, Level)>;

/// Which running processes to change now, and which to put back. A process already at its wanted level is left
/// alone; one whose app is no longer listed is put back; one that has exited is simply forgotten by the caller.
pub fn plan(running: &[(u32, String)], wanted: &BTreeMap<String, Level>, applied: &Applied) -> (Vec<(u32, Level)>, Vec<u32>) {
  let set = running
    .iter()
    .filter_map(|(pid, exe)| wanted.get(exe).map(|l| (*pid, *l)))
    .filter(|(pid, l)| applied.get(pid).is_none_or(|a| a.2 != *l))
    .collect();
  let back = applied.iter().filter(|(pid, a)| !wanted.contains_key(&a.0) && running.iter().any(|(p, _)| p == *pid)).map(|(pid, _)| *pid).collect();
  (set, back)
}

fn throttle(h: windows::Win32::Foundation::HANDLE, on: bool) -> bool {
  // Mask 0 hands the decision back to Windows, which is the default for every process.
  let st = PROCESS_POWER_THROTTLING_STATE {
    Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    ControlMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
    StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
  };
  unsafe { SetProcessInformation(h, ProcessPowerThrottling, &st as *const _ as *const _, std::mem::size_of_val(&st) as u32).is_ok() }
}

/// Sets the level; returns the priority class the process had before.
pub fn apply(pid: u32, level: Level) -> Result<u32, String> {
  unsafe {
    let h = OpenProcess(PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(|e| e.message())?;
    let old = GetPriorityClass(h);
    let class = if level == Level::Below { BELOW_NORMAL_PRIORITY_CLASS } else { IDLE_PRIORITY_CLASS };
    let ok = SetPriorityClass(h, class).is_ok();
    throttle(h, level == Level::Eco);
    let _ = CloseHandle(h);
    if ok && old != 0 { Ok(old) } else { Err("Windows refused the change.".into()) }
  }
}

/// Puts a process back: its old priority class, and power throttling left to Windows.
pub fn revert(pid: u32, old_class: u32) -> bool {
  unsafe {
    let Ok(h) = OpenProcess(PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
    let ok = SetPriorityClass(h, PROCESS_CREATION_FLAGS(old_class)).is_ok();
    throttle(h, false);
    let _ = CloseHandle(h);
    ok
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn plans_changes() {
    let running = vec![(10, "discord.exe".to_string()), (11, "discord.exe".to_string()), (20, "steam.exe".to_string()), (30, "code.exe".to_string())];
    let wanted: BTreeMap<String, Level> = [("discord.exe".to_string(), Level::Eco), ("steam.exe".to_string(), Level::Below)].into();
    let mut applied = Applied::new();
    applied.insert(10, ("discord.exe".into(), 32, Level::Eco));
    applied.insert(20, ("steam.exe".into(), 32, Level::Low));
    applied.insert(30, ("code.exe".into(), 32, Level::Below));
    applied.insert(99, ("gone.exe".into(), 32, Level::Low));
    let (mut set, back) = plan(&running, &wanted, &applied);
    set.sort();
    assert_eq!(set, vec![(11, Level::Eco), (20, Level::Below)], "a new process, and a changed level; pid 10 is already right");
    assert_eq!(back, vec![30], "no longer listed; pid 99 has exited");
  }

  #[test]
  fn levels() {
    assert_eq!(parse_level("eco"), Ok(Some(Level::Eco)));
    assert_eq!(parse_level("normal"), Ok(None));
    assert!(parse_level("realtime").is_err(), "never higher than normal");
  }
}
