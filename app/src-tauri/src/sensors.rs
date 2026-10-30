//! Runs the LibreHardwareMonitor sidecar (resources/sensors/shimmy-sensors.exe, built from C# by build.rs) and keeps
//! its latest readings. The sidecar sits in a job object that kills it when Shimmy exits, and is restarted if it dies.
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
  AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
  JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, Deserialize)]
pub struct Reading {
  pub hw: String,
  #[serde(rename = "type")]
  pub hw_type: String,
  pub sensor: String,
  pub kind: String,
  pub value: Option<f64>,
}

#[derive(Default)]
pub struct Latest {
  pub rows: Vec<Reading>,
  /// When the last line arrived (ms since epoch); 0 if never.
  pub at: u64,
  pub error: Option<String>,
}

pub type SensorState = Arc<Mutex<Latest>>;

/// Readings older than this are treated as missing (the sidecar is not running).
pub const STALE_MS: u64 = 10_000;

pub fn find<'a>(rows: &'a [Reading], hw_type: &str, kind: &str, sensor: &str) -> Option<&'a Reading> {
  rows.iter().find(|r| r.hw_type == hw_type && r.kind == kind && r.sensor == sensor && r.value.is_some())
}

pub fn value(rows: &[Reading], hw_type: &str, kind: &str, sensor: &str) -> Option<f64> {
  find(rows, hw_type, kind, sensor).and_then(|r| r.value)
}

/// The processor temperature: "Core (Tctl/Tdie)", or "Core (Tctl)" on chips that report only that. Every reader
/// (Processor page, heat protection, alerts, the cooling check) goes through here so they always agree.
pub fn cpu_temp(rows: &[Reading]) -> Option<f64> {
  value(rows, "Cpu", "Temperature", "Core (Tctl/Tdie)").or_else(|| value(rows, "Cpu", "Temperature", "Core (Tctl)"))
}

/// How long a good temperature is reused when the sensor briefly returns nonsense.
const HOLD_MS: u64 = 30_000;

/// Ryzen temperature reads sometimes come back as 0 when another program (ASUS's services, for one) talks to the
/// same sensor at that moment. Impossible temperatures become "no reading", and the last good value is held briefly.
#[derive(Default)]
pub struct Steady {
  last: std::collections::HashMap<(String, String), (f64, u64)>,
}

impl Steady {
  pub fn apply(&mut self, mut rows: Vec<Reading>, now: u64) -> Vec<Reading> {
    for r in rows.iter_mut().filter(|r| r.kind == "Temperature") {
      let key = (r.hw.clone(), r.sensor.clone());
      match r.value {
        Some(v) if v > 0.5 && v < 125.0 => {
          self.last.insert(key, (v, now));
        }
        _ => {
          r.value = self.last.get(&key).filter(|(_, at)| now.saturating_sub(*at) <= HOLD_MS).map(|(v, _)| *v);
        }
      }
    }
    rows
  }
}

/// One reading without the running app, for `--report`. The second line is the one used: the processor's power and load
/// need two passes to mean anything. Whatever arrived is returned when time runs out.
pub fn read_once(exe: &std::path::Path, limit: Duration) -> Option<Vec<Reading>> {
  let mut child = Command::new(exe).arg("1000").stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW).spawn().ok()?;
  let out = child.stdout.take()?;
  let (tx, rx) = std::sync::mpsc::channel();
  std::thread::spawn(move || {
    for line in BufReader::new(out).lines().map_while(Result::ok) {
      if let Ok(rows) = serde_json::from_str::<Vec<Reading>>(&line) {
        if tx.send(rows).is_err() {
          return;
        }
      }
    }
  });
  let deadline = std::time::Instant::now() + limit;
  let mut got = None;
  for _ in 0..2 {
    match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
      Ok(rows) => got = Some(rows),
      Err(_) => break,
    }
  }
  let _ = child.kill();
  let _ = child.wait();
  got.map(|rows| Steady::default().apply(rows, crate::store::now_ms()))
}

struct Job(HANDLE);
unsafe impl Send for Job {}

fn kill_on_close_job() -> Option<Job> {
  unsafe {
    let job = CreateJobObjectW(None, None).ok()?;
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32).ok()?;
    Some(Job(job))
  }
}

pub fn start(exe: PathBuf, state: SensorState) {
  // Tauri gives a verbatim \\?\C:\... path; .NET's base directory is unreliable with it.
  let text = exe.to_string_lossy().into_owned();
  let exe = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
  std::thread::spawn(move || {
    // Held for the life of the app; the handle closing at exit kills the sidecar.
    let job = kill_on_close_job();
    let mut backoff = 2;
    loop {
      let spawned = Command::new(&exe)
        .arg("2000")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
      match spawned {
        Err(e) => state.lock().unwrap().error = Some(format!("Could not start the sensor reader: {e}")),
        Ok(mut child) => {
          if let Some(j) = &job {
            unsafe {
              let _ = AssignProcessToJobObject(j.0, HANDLE(child.as_raw_handle()));
            }
          }
          let stderr = child.stderr.take();
          let mut steady = Steady::default();
          if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
              if let Ok(rows) = serde_json::from_str::<Vec<Reading>>(&line) {
                let rows = steady.apply(rows, crate::store::now_ms());
                let mut s = state.lock().unwrap();
                s.rows = rows;
                s.at = crate::store::now_ms();
                s.error = None;
                backoff = 2;
              }
            }
          }
          let _ = child.wait();
          let detail = stderr
            .map(|e| BufReader::new(e).lines().map_while(Result::ok).filter(|l| !l.trim().is_empty()).take(3).collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
          state.lock().unwrap().error = Some(if detail.is_empty() { "The sensor reader stopped.".into() } else { detail });
        }
      }
      std::thread::sleep(Duration::from_secs(backoff));
      backoff = (backoff * 2).min(60);
    }
  });
}

#[cfg(test)]
mod tests {
  use super::*;

  fn temp(v: Option<f64>) -> Vec<Reading> {
    vec![Reading { hw: "Ryzen".into(), hw_type: "Cpu".into(), sensor: "Core (Tctl/Tdie)".into(), kind: "Temperature".into(), value: v }]
  }

  #[test]
  fn zero_temperatures_are_not_shown() {
    let mut s = Steady::default();
    let read = |s: &mut Steady, v, at| s.apply(temp(v), at)[0].value;
    assert_eq!(read(&mut s, Some(0.0), 0), None, "a 0 before any good reading is no reading");
    assert_eq!(read(&mut s, Some(61.25), 1_000), Some(61.25));
    assert_eq!(read(&mut s, Some(0.0), 3_000), Some(61.25), "a glitch holds the last good value");
    assert_eq!(read(&mut s, None, 20_000), Some(61.25));
    assert_eq!(read(&mut s, Some(0.0), 40_000), None, "after 30 s without a good value, show nothing");
    assert_eq!(read(&mut s, Some(250.0), 41_000), None, "impossible values are dropped too");
  }

  #[test]
  fn cpu_temperature_falls_back_to_tctl() {
    assert_eq!(cpu_temp(&temp(Some(61.0))), Some(61.0));
    let tctl = vec![Reading { hw: "Ryzen".into(), hw_type: "Cpu".into(), sensor: "Core (Tctl)".into(), kind: "Temperature".into(), value: Some(58.0) }];
    assert_eq!(cpu_temp(&tctl), Some(58.0), "chips that report only Tctl");
    assert_eq!(cpu_temp(&[]), None);
  }
}
