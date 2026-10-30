//! A one-page text snapshot of this PC: `shimmy-optimizer.exe --report`, and Save a report on the System page. Read only.
//! No serial numbers, user name or addresses, so it is safe to paste anywhere. Plain ASCII of its own, because a console
//! or a PowerShell redirect can garble anything else.
use crate::net::{self, Place};
use crate::procs::{Mem, Proc};
use crate::sensors::{self, Reading};
use crate::storage::Drive;
use crate::system::Battery;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::time::Duration;

pub struct Snapshot {
  pub when: String,
  pub hw: Option<Value>,
  pub cpu_load: Option<f64>,
  pub rows: Option<Vec<Reading>>,
  pub fans: Vec<crate::cpu::Fan>,
  pub mem: Mem,
  pub drives: Vec<Drive>,
  pub battery: Battery,
  pub uptime_s: u64,
  pub power_mode: Option<String>,
  pub procs: Vec<Proc>,
  pub conns: Option<Vec<net::Conn>>,
}

fn gb(b: u64) -> String {
  format!("{:.1} GB", b as f64 / 1_073_741_824.0)
}

fn squeeze(s: &str) -> String {
  s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_at(v: &Value, path: &[&str]) -> String {
  squeeze(path.iter().fold(v, |v, k| &v[*k]).as_str().unwrap_or(""))
}

/// The apps using the most memory, adding up each app's processes.
pub fn top_apps(procs: &[Proc], n: usize) -> Vec<(String, u64)> {
  let mut by: BTreeMap<String, u64> = BTreeMap::new();
  for p in procs {
    *by.entry(p.name.to_lowercase()).or_default() += p.ws;
  }
  let mut top: Vec<_> = by.into_iter().collect();
  top.sort_by_key(|t| std::cmp::Reverse(t.1));
  top.truncate(n);
  top
}

fn uptime(s: u64) -> String {
  match (s / 86_400, s % 86_400 / 3600, s % 3600 / 60) {
    (0, 0, m) => format!("{m} min"),
    (0, h, m) => format!("{h} h {m} min"),
    (d, h, _) => format!("{d} d {h} h"),
  }
}

pub fn text(s: &Snapshot) -> String {
  let mut out = vec![format!("Shimmy Optimizer {} report, {}", crate::update::CURRENT, s.when), String::new()];
  let mut add = |k: &str, v: String| out.push(format!("{:<12}{v}", format!("{k}:")));
  let empty = Value::Null;
  let hw = s.hw.as_ref().unwrap_or(&empty);
  let model = format!("{} {}", text_at(hw, &["system", "Manufacturer"]), text_at(hw, &["system", "Model"]));
  add("Computer", if model.trim().is_empty() { "not read".into() } else { model.trim().to_string() });
  let (caption, version, build) = (text_at(hw, &["os", "Caption"]), text_at(hw, &["release", "DisplayVersion"]), text_at(hw, &["os", "BuildNumber"]));
  add("Windows", if caption.is_empty() { "not read".into() } else { format!("{caption} {version}, build {build}") });
  add("Running", format!("{} since the last restart", uptime(s.uptime_s)));
  let cpu = hw["cpu"][0]["Name"].as_str().map_or(String::new(), |n| format!("{}: ", squeeze(n)));
  let rows = s.rows.as_deref().unwrap_or(&[]);
  let temp = sensors::cpu_temp(rows).map_or("temperature not read".to_string(), |t| format!("{t:.0} C"));
  // Without administrator rights the sensor reader says 0 W, which only means it could not tell.
  let watts = sensors::value(rows, "Cpu", "Power", "Package").filter(|w| *w > 0.0).map_or(String::new(), |w| format!(", {w:.0} W"));
  let load = s.cpu_load.map_or("load not read".to_string(), |l| format!("{l:.0}% load"));
  add("Processor", format!("{cpu}{load}, {temp}{watts}"));
  let gpus: Vec<String> = hw["gpu"].as_array().map(|g| g.iter().filter_map(|x| x["Name"].as_str()).map(squeeze).collect()).unwrap_or_default();
  add("Graphics", if gpus.is_empty() { "not read".into() } else { gpus.join(", ") });
  add("Memory", format!("{} of {} in use ({}%)", gb(s.mem.used_mb * 1_048_576), gb(s.mem.total_mb * 1_048_576), s.mem.pct));
  for d in &s.drives {
    add("Drive", format!("{} {} free of {}", d.letter, gb(d.free), gb(d.total)));
  }
  if s.battery.present {
    let how = if s.battery.charging { "charging" } else if s.battery.plugged { "on the charger" } else { "on battery" };
    add("Battery", format!("{}, {how}", s.battery.pct.map_or("level not read".into(), |p| format!("{p}%"))));
  }
  if let Some(mode) = &s.power_mode {
    add("Power mode", mode.clone());
  }
  if !s.fans.is_empty() {
    add("Fans", s.fans.iter().map(|f| format!("{} {} RPM", f.name, f.rpm)).collect::<Vec<_>>().join(", "));
  }
  if let Some(c) = &s.conns {
    let on_net = c.iter().filter(|x| x.place == Some(Place::Internet) && x.state == "Connected").count();
    let waits = c.iter().filter(|x| x.state == "Listening" && x.place.is_none()).count();
    let s_of = |n: usize| if n == 1 { "" } else { "s" };
    add("Network", format!("{on_net} connection{} to the internet, {waits} port{} listening", s_of(on_net), s_of(waits)));
  }
  let top = top_apps(&s.procs, 5);
  if !top.is_empty() {
    out.push(String::new());
    out.push("Using the most memory:".into());
    out.extend(top.iter().map(|(name, ws)| format!("  {:<32}{}", name, gb(*ws))));
  }
  out.push(String::new());
  out.join("\r\n")
}

/// Whether anything is listening to stdout. Shimmy is a windowed program, so it has no console of its own: a redirect or
/// a pipe is already there, and otherwise it borrows the console of the terminal that started it. None of that exists
/// when it was started elevated from a non-elevated terminal, or from the Run box.
fn can_print() -> bool {
  use windows::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_UNKNOWN};
  use windows::Win32::System::Console::{AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE};
  unsafe {
    let usable = || GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null() && GetFileType(h) != FILE_TYPE_UNKNOWN);
    // Writing to a missing handle reports success and goes nowhere, so an attached console must really give one.
    usable() || (AttachConsole(ATTACH_PARENT_PROCESS).is_ok() && usable())
  }
}

/// Saves the report as a text file and shows it: `dir` is Downloads.
pub fn save(dir: &std::path::Path, stamp: &str, text: &str) -> Result<(String, std::path::PathBuf), String> {
  let name = format!("Shimmy report {stamp}.txt");
  let file = dir.join(&name);
  crate::store::write_new(dir, &name, text.as_bytes()).map_err(|e| format!("Could not save it to Downloads: {e}"))?;
  let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", file.display())).spawn();
  Ok((name, file))
}

/// `--report`: Shimmy need not be running, so this reads everything itself, then prints, or saves when it cannot print.
pub fn cli() -> i32 {
  let printing = can_print();
  let mut sys = sysinfo::System::new();
  sys.refresh_cpu_usage();
  std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(500)));
  sys.refresh_cpu_usage();
  let cpu_load = Some(sys.global_cpu_usage() as f64);
  let exe = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("sensors").join("shimmy-sensors.exe")));
  let fans = crate::asus::Atk::open().map_or(Vec::new(), |atk| crate::fans_of(&atk));
  let snap = Snapshot {
    when: crate::local_clock(),
    hw: crate::hw::read().ok(),
    cpu_load,
    rows: exe.and_then(|e| sensors::read_once(&e, Duration::from_secs(12))),
    fans,
    mem: crate::system::mem_now(),
    drives: crate::storage::drives(),
    battery: crate::system::battery_now(),
    uptime_s: sysinfo::System::uptime(),
    power_mode: crate::power::current().map(crate::power::label).map(String::from),
    procs: crate::system::list_procs(&mut sys),
    conns: net::list().ok(),
  };
  let body = text(&snap);
  if printing && writeln!(std::io::stdout(), "\r\n{body}").and_then(|_| std::io::stdout().flush()).is_ok() {
    return 0;
  }
  match crate::clean::known("Downloads").ok_or_else(|| "Windows did not say where Downloads is.".to_string()).and_then(|d| save(&d, &crate::local_stamp(), &body)) {
    Ok(_) => 0,
    Err(_) => 1,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn proc(name: &str, ws: u64) -> Proc {
    serde_json::from_value(json!({ "pid": 1, "ppid": 0, "name": name, "ws": ws, "cmd": "", "path": "", "title": "" })).unwrap()
  }

  fn snap() -> Snapshot {
    Snapshot {
      when: "2026-10-30 10:00".into(),
      hw: None,
      cpu_load: None,
      rows: None,
      fans: Vec::new(),
      mem: Mem { total_mb: 16_000, free_mb: 8_000, used_mb: 8_000, pct: 50 },
      drives: Vec::new(),
      battery: Battery { present: false, pct: None, plugged: true, charging: false, seconds_left: None },
      uptime_s: 3_700,
      power_mode: None,
      procs: Vec::new(),
      conns: None,
    }
  }

  #[test]
  fn top_apps_add_up_each_apps_processes() {
    let top = top_apps(&[proc("Chrome.exe", 100), proc("chrome.exe", 300), proc("a.exe", 200), proc("b.exe", 1)], 2);
    assert_eq!(top, [("chrome.exe".to_string(), 400), ("a.exe".to_string(), 200)]);
  }

  #[test]
  fn uptime_reads_plainly() {
    assert_eq!((uptime(59), uptime(3_700), uptime(180_000)), ("0 min".to_string(), "1 h 1 min".to_string(), "2 d 2 h".to_string()));
  }

  #[test]
  fn what_could_not_be_read_says_so() {
    let t = text(&snap());
    assert!(t.contains("Computer:   not read"));
    assert!(t.contains("load not read, temperature not read"));
    assert!(t.contains("7.8 GB of 15.6 GB in use (50%)"), "{t}");
    assert!(!t.contains("Battery:"), "no battery, no line");
    assert!(!t.contains("Network:"));
    assert!(t.is_ascii());
  }

  #[test]
  fn a_full_report_fits_one_page_and_holds_no_addresses() {
    let mut s = snap();
    s.hw = Some(json!({
      "system": { "Manufacturer": "ASUS", "Model": "Vivobook  M3500QC" },
      "os": { "Caption": "Windows 11 Pro", "BuildNumber": "26200" },
      "release": { "DisplayVersion": "25H2" },
      "cpu": [{ "Name": "AMD Ryzen 7 5800HS  " }],
      "gpu": [{ "Name": "RTX 3050" }, { "Name": "Radeon" }],
    }));
    s.cpu_load = Some(12.4);
    s.rows = Some(vec![Reading { hw: "R".into(), hw_type: "Cpu".into(), sensor: "Core (Tctl/Tdie)".into(), kind: "Temperature".into(), value: Some(61.2) }]);
    s.fans = vec![crate::cpu::Fan { name: "Processor fan".into(), rpm: 2400 }];
    s.battery = Battery { present: true, pct: Some(80), plugged: true, charging: false, seconds_left: None };
    s.procs = vec![proc("chrome.exe", 1_073_741_824)];
    s.power_mode = Some("Balanced".into());
    s.conns = Some(vec![net::Conn { proto: "TCP", pid: 1, local: "192.168.1.5".into(), local_port: 5, remote: Some("8.8.8.8".into()), remote_port: Some(443), place: Some(Place::Internet), state: "Connected" }]);
    let t = text(&s);
    assert!(t.contains("Computer:   ASUS Vivobook M3500QC"), "{t}");
    assert!(t.contains("Windows 11 Pro 25H2, build 26200"));
    assert!(t.contains("AMD Ryzen 7 5800HS: 12% load, 61 C"));
    assert!(t.contains("RTX 3050, Radeon"));
    assert!(t.contains("80%, on the charger"));
    assert!(t.contains("Processor fan 2400 RPM"));
    assert!(t.contains("1 connection to the internet, 0 ports listening"));
    assert!(t.contains("chrome.exe"));
    assert!(!t.contains("8.8.8.8") && !t.contains("192.168"), "addresses stay out");
    assert!(t.lines().count() < 40);
  }
}
