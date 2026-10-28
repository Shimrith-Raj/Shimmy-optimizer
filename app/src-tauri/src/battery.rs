//! The Battery page: live charge and wear (sensor sidecar), cycle count and capacity history (Windows' battery
//! report), and ASUS charge care (the firmware charge limit).
use crate::sensors::{value, Reading};
use regex::Regex;
use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
  pub date: String,
  pub full_mwh: u64,
  pub design_mwh: u64,
  pub cycles: Option<u32>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
  pub manufacturer: Option<String>,
  pub chemistry: Option<String>,
  pub design_mwh: Option<u64>,
  pub full_mwh: Option<u64>,
  pub cycles: Option<u32>,
  /// Windows' estimate of active use on a full charge, in minutes.
  pub full_runtime_min: Option<u32>,
  pub history: Vec<HistoryPoint>,
}

static ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(\w+)="([^"]*)""#).unwrap());
static ENTRY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<HistoryEntry\s([^>]*)/?>").unwrap());
static DURATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^P(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:[\d.]+S)?)?$").unwrap());

fn element(xml: &str, tag: &str) -> Option<String> {
  let open = format!("<{tag}>");
  let start = xml.find(&open)? + open.len();
  let end = start + xml[start..].find(&format!("</{tag}>"))?;
  Some(xml[start..end].trim().to_string()).filter(|s| !s.is_empty())
}

/// ISO 8601 duration such as PT4H53M4S, in whole minutes.
fn minutes(d: &str) -> Option<u32> {
  let c = DURATION.captures(d)?;
  let n = |i| c.get(i).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(0);
  Some(n(1) * 1440 + n(2) * 60 + n(3))
}

pub fn parse(xml: &str) -> Report {
  // The first <Batteries> block holds the battery's own figures.
  let batteries = xml.find("<Batteries>").map(|i| &xml[i..]).unwrap_or(xml);
  let num = |tag| element(batteries, tag).and_then(|v| v.parse::<u64>().ok());
  let history: Vec<HistoryPoint> = ENTRY
    .captures_iter(xml)
    .filter_map(|c| {
      let attrs: Vec<(String, String)> = ATTR.captures_iter(&c[1]).map(|a| (a[1].to_string(), a[2].to_string())).collect();
      let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
      Some(HistoryPoint {
        date: get("LocalStartDate")?.chars().take(10).collect(),
        full_mwh: get("FullChargeCapacity")?.parse().ok()?,
        design_mwh: get("DesignCapacity")?.parse().ok()?,
        cycles: get("CycleCount").and_then(|v| v.parse().ok()),
      })
    })
    .filter(|h| h.full_mwh > 0 && h.design_mwh > 0)
    .collect();
  let last_runtime = ENTRY.captures_iter(xml).last().and_then(|c| {
    ATTR.captures_iter(&c[1]).find(|a| &a[1] == "EstimatedFullChargeActiveTime").and_then(|a| minutes(&a[2]))
  });
  Report {
    manufacturer: element(batteries, "Manufacturer"),
    chemistry: element(batteries, "Chemistry"),
    design_mwh: num("DesignCapacity"),
    full_mwh: num("FullChargeCapacity"),
    cycles: element(batteries, "CycleCount").and_then(|v| v.parse().ok()),
    full_runtime_min: last_runtime,
    history,
  }
}

/// Runs `powercfg /batteryreport` to a temporary file and reads it (about two seconds).
pub fn report() -> Result<Report, String> {
  let path = std::env::temp_dir().join(format!("shimmy-battery-{}.xml", std::process::id()));
  let out = Command::new("powercfg.exe")
    .args(["/batteryreport", "/xml", "/output"])
    .arg(&path)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .creation_flags(CREATE_NO_WINDOW)
    .status()
    .map_err(|e| format!("Could not run powercfg: {e}"))?;
  let xml = std::fs::read_to_string(&path).map_err(|_| format!("powercfg did not write a battery report (exit {out})."))?;
  let _ = std::fs::remove_file(&path);
  Ok(parse(&xml))
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Status {
  pub level: &'static str,
  pub text: String,
}

pub fn status(health: Option<f64>, cycles: Option<u32>) -> Status {
  let s = |level, text: String| Status { level, text };
  let after = cycles.map(|c| format!(" after {c} charge cycles")).unwrap_or_default();
  // Judge the whole % the text shows, so 79.6 reads "80%" and is "good".
  match health.map(f64::round) {
    None => s("unknown", "Windows did not report the battery's capacity.".into()),
    Some(h) if h >= 80.0 => s("good", format!("Healthy. It holds {h:.0}% of the charge it held when new{after}.")),
    Some(h) if h >= 60.0 => s("warn", format!("Worn. It holds {h:.0}% of its original charge{after}, which is normal wear but means shorter days on battery.")),
    Some(h) => s("bad", format!("Heavily worn. It holds only {h:.0}% of its original charge{after}. A replacement battery would bring back full runtime.")),
  }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BatteryView {
  pub present: bool,
  pub charge_pct: Option<f64>,
  pub plugged: bool,
  pub charging: bool,
  /// Watts; positive while charging, negative while discharging.
  pub rate_w: Option<f64>,
  pub voltage: Option<f64>,
  pub design_mwh: Option<u64>,
  pub full_mwh: Option<u64>,
  pub remaining_mwh: Option<u64>,
  pub health_pct: Option<f64>,
  pub cycles: Option<u32>,
  pub chemistry: Option<String>,
  pub manufacturer: Option<String>,
  pub seconds_left: Option<u32>,
  pub full_runtime_min: Option<u32>,
  pub history: Vec<HistoryPoint>,
  pub status: Status,
  pub charge_limit: Option<u32>,
  pub limit_supported: bool,
}

/// Full-charge capacity as a % of design capacity, from the sensors and from Windows' battery report otherwise.
/// The one place health is worked out, for the Battery page and the Home tile.
pub fn health(rows: Option<&[Reading]>, report: Option<&Report>) -> Option<f64> {
  let rows = rows.unwrap_or(&[]);
  let energy = |s: &str| value(rows, "Battery", "Energy", s).map(|v| v.round() as u64);
  let design = energy("Designed Capacity").or(report.and_then(|r| r.design_mwh));
  let full = energy("Fully-Charged Capacity").or(report.and_then(|r| r.full_mwh));
  match (full, design) {
    (Some(f), Some(d)) if d > 0 => Some(f as f64 / d as f64 * 100.0),
    _ => None,
  }
}

pub fn build(live: &crate::system::Battery, rows: Option<&[Reading]>, report: Option<&Report>, limit: Option<u32>, limit_supported: bool) -> BatteryView {
  let rows = rows.unwrap_or(&[]);
  let energy = |s: &str| value(rows, "Battery", "Energy", s).map(|v| v.round() as u64);
  let design = energy("Designed Capacity").or(report.and_then(|r| r.design_mwh));
  let full = energy("Fully-Charged Capacity").or(report.and_then(|r| r.full_mwh));
  let health = health(Some(rows), report);
  let rate = value(rows, "Battery", "Power", "Charge Rate").or_else(|| value(rows, "Battery", "Power", "Discharge Rate").map(|w| -w));
  let cycles = report.and_then(|r| r.cycles);
  BatteryView {
    present: live.present,
    // Windows' own whole % first, so the page shows the same number as the taskbar.
    charge_pct: live.pct.map(f64::from).or(value(rows, "Battery", "Level", "Charge Level")),
    plugged: live.plugged,
    charging: live.charging || rate.is_some_and(|w| w > 0.5),
    rate_w: rate,
    voltage: value(rows, "Battery", "Voltage", "Voltage"),
    design_mwh: design,
    full_mwh: full,
    remaining_mwh: energy("Remaining Capacity"),
    health_pct: health,
    cycles,
    chemistry: report.and_then(|r| r.chemistry.clone()),
    manufacturer: report.and_then(|r| r.manufacturer.clone()),
    seconds_left: live.seconds_left,
    full_runtime_min: report.and_then(|r| r.full_runtime_min),
    history: report.map(|r| r.history.clone()).unwrap_or_default(),
    status: status(health, cycles),
    charge_limit: limit,
    limit_supported,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reads_the_battery_report() {
    let r = parse(include_str!("../tests/fixtures/battery-report.xml"));
    assert_eq!(r.design_mwh, Some(63041));
    assert_eq!(r.full_mwh, Some(46250));
    assert_eq!(r.cycles, Some(312));
    assert!(r.history.len() >= 40);
    assert_eq!(r.history[0].date, "2025-11-25");
    assert_eq!(r.history[0].full_mwh, 54756);
    assert!(r.full_runtime_min.unwrap() > 60);
  }

  #[test]
  fn durations() {
    assert_eq!(minutes("PT4H53M4S"), Some(293));
    assert_eq!(minutes("P1DT16H31M13S"), Some(2431));
    assert_eq!(minutes("PT0S"), Some(0));
  }

  #[test]
  fn wear_wording() {
    assert_eq!(status(Some(73.4), Some(312)).level, "warn");
    assert!(status(Some(73.4), Some(312)).text.contains("73% of its original charge after 312 charge cycles"));
    assert_eq!(status(Some(91.0), None).level, "good");
    assert_eq!(status(Some(52.0), None).level, "bad");
    // The level follows the whole % the text shows.
    assert_eq!(status(Some(79.6), None).level, "good");
    assert_eq!(status(None, None).level, "unknown");
  }
}
