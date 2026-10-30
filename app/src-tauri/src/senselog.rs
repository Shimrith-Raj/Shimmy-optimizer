//! Sensor log: chosen readings written to a CSV in Downloads at a set interval, until you press Stop or Shimmy exits.
//! Every row is flushed, so a crash loses nothing. The graphics chips are left out, as in the sensor reader itself:
//! asking keeps the RTX awake.
use crate::sensors::Reading;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MIN_SECS: u32 = 2;
pub const MAX_SECS: u32 = 3600;
pub const MAX_COLUMNS: usize = 40;
/// A forgotten log stops here rather than slowly filling the drive.
pub const MAX_BYTES: u64 = 100 * 1_048_576;
const MAX_KEY: usize = 200;
/// The `hw` of readings Shimmy adds itself, besides the sensor reader's.
pub const OWN_HW: &str = "This PC";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
  pub interval_s: u32,
  /// Empty means nothing chosen yet, so the page ticks `default_key`s.
  pub chosen: Vec<String>,
}

impl Default for Prefs {
  fn default() -> Self {
    Prefs { interval_s: 5, chosen: Vec::new() }
  }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub enum Why {
  #[serde(rename = "you pressed Stop")]
  Asked,
  #[serde(rename = "it reached 100 MB")]
  Full,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Done {
  pub name: String,
  pub path: String,
  pub rows: u64,
  pub bytes: u64,
  pub why: Why,
}

pub fn clamp_secs(s: u32) -> u32 {
  s.clamp(MIN_SECS, MAX_SECS)
}

/// Cuts a list from settings.json or the page down to something sane: no blanks, no repeats, no absurd lengths.
fn tidy(keys: &[String]) -> Vec<String> {
  let mut seen = HashSet::new();
  keys.iter().map(|k| k.trim()).filter(|k| !k.is_empty() && k.len() <= MAX_KEY && seen.insert(k.to_string())).take(MAX_COLUMNS).map(String::from).collect()
}

pub fn clamp(p: &mut Prefs) {
  p.interval_s = clamp_secs(p.interval_s);
  p.chosen = tidy(&p.chosen);
}

/// The gate for Start. A key that no sensor has only gives an empty column, so keys are not checked against the sensors.
pub fn check_keys(keys: &[String]) -> Result<Vec<String>, String> {
  if tidy(keys).is_empty() {
    return Err("Tick at least one sensor to log.".into());
  }
  if keys.len() > MAX_COLUMNS {
    return Err(format!("A log can hold {MAX_COLUMNS} sensors at most, and {} are ticked.", keys.len()));
  }
  Ok(tidy(keys))
}

pub fn key(r: &Reading) -> String {
  format!("{}|{}|{}", r.hw, r.sensor, r.kind)
}

pub fn unit(kind: &str) -> &'static str {
  match kind {
    "Temperature" => "°C",
    "Load" | "Level" | "Control" => "%",
    "Clock" | "Frequency" => "MHz",
    "Power" => "W",
    "Voltage" => "V",
    "Current" => "A",
    "Fan" => "RPM",
    "Energy" => "mWh",
    "Data" => "GB",
    "SmallData" => "MB",
    "Throughput" => "B/s",
    "TimeSpan" => "s",
    _ => "",
  }
}

/// A column's heading, from the key alone so a sensor missing right now still gets one.
pub fn heading(key: &str) -> String {
  let mut parts = key.rsplitn(3, '|');
  let (kind, sensor, hw) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
  let unit = unit(kind);
  let name = if hw.is_empty() { sensor.to_string() } else { format!("{hw} {sensor}") };
  if unit.is_empty() { name } else { format!("{name} ({unit})") }
}

/// The readings ticked for a log nobody has set up: what most people want to see, not all hundred.
pub fn default_key(r: &Reading) -> bool {
  r.hw == OWN_HW
    || (r.hw_type == "Cpu" && ((r.kind == "Temperature" && r.sensor.starts_with("Core (Tctl")) || (r.kind == "Power" && r.sensor == "Package")))
}

/// One line: local time, then each key's value, or an empty cell when that sensor has nothing right now. Numbers and
/// Shimmy's own clock only, so there is nothing here for Excel to take as a formula.
pub fn line(time: &str, keys: &[String], rows: &[Reading]) -> String {
  let mut cells = vec![time.to_string()];
  cells.extend(keys.iter().map(|k| rows.iter().find(|r| key(r) == *k).and_then(|r| r.value).map_or(String::new(), |v| ((v * 100.0).round() / 100.0).to_string())));
  crate::store::csv_row(&cells.iter().map(String::as_str).collect::<Vec<_>>())
}

pub fn header(keys: &[String]) -> String {
  let mut cells = vec!["time".to_string()];
  cells.extend(keys.iter().map(|k| heading(k)));
  crate::store::export_row(&cells.iter().map(String::as_str).collect::<Vec<_>>())
}

pub fn stop_reason(stop: bool, bytes: u64) -> Option<Why> {
  if stop {
    Some(Why::Asked)
  } else if bytes >= MAX_BYTES {
    Some(Why::Full)
  } else {
    None
  }
}

/// When the next row is due. After a sleep or a long stall it starts again from now, rather than writing a burst of rows
/// to catch up.
pub fn next_due(due: u64, now: u64, interval_ms: u64) -> u64 {
  let next = due + interval_ms;
  if next <= now { now + interval_ms } else { next }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn r(hw: &str, ty: &str, sensor: &str, kind: &str, value: Option<f64>) -> Reading {
    Reading { hw: hw.into(), hw_type: ty.into(), sensor: sensor.into(), kind: kind.into(), value }
  }

  fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
  }

  #[test]
  fn intervals_and_lists_are_kept_sane() {
    assert_eq!((clamp_secs(0), clamp_secs(5), clamp_secs(99_999)), (2, 5, 3600));
    let long = "x".repeat(300);
    let mut p = Prefs { interval_s: 1, chosen: strs(&["a", " a ", "", &long, "b"]) };
    clamp(&mut p);
    assert_eq!((p.interval_s, p.chosen), (2, strs(&["a", "b"])));
    let mut many = Prefs { interval_s: 5, chosen: (0..99).map(|i| i.to_string()).collect() };
    clamp(&mut many);
    assert_eq!(many.chosen.len(), MAX_COLUMNS);
  }

  #[test]
  fn start_needs_a_sensible_list() {
    assert!(check_keys(&[]).is_err());
    assert!(check_keys(&strs(&["", "  "])).is_err());
    let too_many: Vec<String> = (0..41).map(|i| i.to_string()).collect();
    assert!(check_keys(&too_many).unwrap_err().contains("41"));
    assert_eq!(check_keys(&strs(&["a", "a", "b"])).unwrap(), strs(&["a", "b"]));
  }

  #[test]
  fn headings_come_from_the_key() {
    assert_eq!(heading("Ryzen 7|Core (Tctl/Tdie)|Temperature"), "Ryzen 7 Core (Tctl/Tdie) (°C)");
    assert_eq!(heading("This PC|Memory in use|Load"), "This PC Memory in use (%)");
    assert_eq!(heading("A|B|Weird"), "A B", "an unknown kind has no unit");
    assert_eq!(heading("odd|hw|name|Fan"), "odd|hw name (RPM)", "a bar in the hardware name");
  }

  #[test]
  fn rows_keep_their_columns_when_a_sensor_is_missing() {
    let keys = strs(&["C|T|Temperature", "C|P|Power", "gone|x|Load"]);
    let rows = [r("C", "Cpu", "P", "Power", Some(12.5049)), r("C", "Cpu", "T", "Temperature", None)];
    assert_eq!(line("2026-10-30 10:00:00", &keys, &rows), "\"2026-10-30 10:00:00\",\"\",\"12.5\",\"\"\n", "two decimals at most");
  }

  #[test]
  fn headers_are_safe_in_excel() {
    assert_eq!(header(&strs(&["=cmd|x|Fan"])), "\"time\",\"'=cmd x (RPM)\"\n");
  }

  #[test]
  fn defaults_are_the_few_that_matter() {
    assert!(default_key(&r(OWN_HW, "Shimmy", "Memory in use", "Load", None)));
    assert!(default_key(&r("Ryzen", "Cpu", "Core (Tctl/Tdie)", "Temperature", None)));
    assert!(default_key(&r("Ryzen", "Cpu", "Package", "Power", None)));
    assert!(!default_key(&r("Ryzen", "Cpu", "Core #1", "Load", None)));
    assert!(!default_key(&r("NVMe", "Storage", "Package", "Power", None)));
  }

  #[test]
  fn a_log_stops_when_asked_or_full() {
    assert_eq!(stop_reason(false, 0), None);
    assert_eq!(stop_reason(true, 0), Some(Why::Asked));
    assert_eq!(stop_reason(false, MAX_BYTES - 1), None);
    assert_eq!(stop_reason(false, MAX_BYTES), Some(Why::Full));
  }

  #[test]
  fn a_stall_does_not_write_a_burst_of_rows() {
    assert_eq!(next_due(10_000, 10_100, 5_000), 15_000, "on time keeps the rhythm");
    assert_eq!(next_due(10_000, 90_000, 5_000), 95_000, "after a sleep it starts from now");
  }
}
