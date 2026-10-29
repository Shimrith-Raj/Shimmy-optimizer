//! Heat protection: when the processor stays hot, Shimmy turns CPU boost off (Windows' processor boost mode, AC and DC)
//! and turns it back on once it has cooled. Measured on this laptop: boost off runs about 8 °C cooler under full load
//! for about 0.5 % less average clock (PLAN.md, Phase 9). The decisions are pure functions; lib.rs feeds them the
//! temperature every 3 s and carries out what they return.
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::process::Command;

/// Boost goes off after this long at or above `HOT_C`.
pub const HOT_C: f64 = 90.0;
const HOT_MS: u64 = 30_000;
/// Boost comes back after this long at or below `COOL_C`.
pub const COOL_C: f64 = 75.0;
const COOL_MS: u64 = 2 * 60_000;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Windows' boost mode values for a power plan, saved before Shimmy turns boost off so they can be put back exactly.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SavedBoost {
  /// The power plan Shimmy changed; restored there even if another plan is active by then.
  pub scheme: String,
  pub ac: u32,
  pub dc: u32,
  /// When boost went off (ms).
  pub since: u64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct HeatPrefs {
  pub enabled: bool,
  /// Set while Shimmy has boost off. Kept by Shimmy, not by the page; also how a restart finds boost left off.
  pub saved: Option<SavedBoost>,
}

impl Default for HeatPrefs {
  fn default() -> Self {
    HeatPrefs { enabled: true, saved: None }
  }
}

#[derive(Debug, PartialEq)]
pub enum Action {
  BoostOff,
  BoostOn,
}

#[derive(Default)]
pub struct Watch {
  hot_since: Option<u64>,
  cool_since: Option<u64>,
}

impl Watch {
  /// `temp` is None when the sensors are not delivering; that counts as cool, so boost is never left off blind.
  pub fn step(&mut self, enabled: bool, boost_off: bool, temp: Option<f64>, now: u64) -> Option<Action> {
    if !enabled {
      self.hot_since = None;
      self.cool_since = None;
      return boost_off.then_some(Action::BoostOn);
    }
    if !boost_off {
      self.cool_since = None;
      if temp.is_some_and(|t| t >= HOT_C) {
        let since = *self.hot_since.get_or_insert(now);
        if now.saturating_sub(since) >= HOT_MS {
          self.hot_since = None;
          return Some(Action::BoostOff);
        }
      } else {
        self.hot_since = None;
      }
      return None;
    }
    self.hot_since = None;
    if temp.is_none_or(|t| t <= COOL_C) {
      let since = *self.cool_since.get_or_insert(now);
      if now.saturating_sub(since) >= COOL_MS {
        self.cool_since = None;
        return Some(Action::BoostOn);
      }
    } else {
      self.cool_since = None;
    }
    None
  }
}

fn powercfg(args: &[&str]) -> Result<String, String> {
  let out = Command::new("powercfg.exe").args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
  if !out.status.success() {
    return Err(format!("powercfg {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stdout).trim()));
  }
  Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The active power plan's GUID, from "Power Scheme GUID: 381b4222-... (Balanced)".
fn active_scheme(text: &str) -> Option<String> {
  let after = text.split("GUID:").nth(1)?.trim();
  let guid = after.split_whitespace().next()?;
  valid_scheme(guid).then(|| guid.to_string())
}

/// A power plan GUID as powercfg prints it (8-4-4-4-12 hex digits). Anything else is never passed to powercfg.
pub fn valid_scheme(g: &str) -> bool {
  let parts: Vec<&str> = g.split('-').collect();
  parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12]) && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The AC and DC values from `powercfg /qh ... PERFBOOSTMODE` ("Current AC Power Setting Index: 0x00000002").
fn boost_values(text: &str) -> Option<(u32, u32)> {
  let find = |which: &str| {
    text.lines()
      .find(|l| l.contains(&format!("Current {which} Power Setting Index")))
      .and_then(|l| l.rsplit("0x").next())
      .and_then(|h| u32::from_str_radix(h.trim(), 16).ok())
  };
  Some((find("AC")?, find("DC")?))
}

fn set_boost(scheme: &str, ac: u32, dc: u32) -> Result<(), String> {
  powercfg(&["/setacvalueindex", scheme, "SUB_PROCESSOR", "PERFBOOSTMODE", &ac.to_string()])?;
  powercfg(&["/setdcvalueindex", scheme, "SUB_PROCESSOR", "PERFBOOSTMODE", &dc.to_string()])?;
  // Values written to the active plan take effect only when it is applied again.
  if active_scheme(&powercfg(&["/getactivescheme"])?).as_deref() == Some(scheme) {
    powercfg(&["/setactive", scheme])?;
  }
  Ok(())
}

/// Turns boost off on the active plan and returns what to put back later.
pub fn boost_off(now: u64) -> Result<SavedBoost, String> {
  let scheme = active_scheme(&powercfg(&["/getactivescheme"])?).ok_or("Windows did not name its power plan.")?;
  let (ac, dc) = boost_values(&powercfg(&["/qh", &scheme, "SUB_PROCESSOR", "PERFBOOSTMODE"])?).ok_or("Windows did not report the boost setting.")?;
  // If a later step fails after the AC value was written, the old values go back at once: nothing is saved to restore
  // them from otherwise.
  if let Err(e) = set_boost(&scheme, 0, 0) {
    let _ = set_boost(&scheme, ac, dc);
    return Err(e);
  }
  Ok(SavedBoost { scheme, ac, dc, since: now })
}

/// Puts back the values saved when boost went off, on the plan they came from.
pub fn restore(saved: &SavedBoost) -> Result<(), String> {
  set_boost(&saved.scheme, saved.ac, saved.dc)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn boost_goes_off_after_30_s_hot_and_back_after_2_min_cool() {
    let mut w = Watch::default();
    assert_eq!(w.step(true, false, Some(91.0), 0), None);
    assert_eq!(w.step(true, false, Some(92.0), 27_000), None);
    assert_eq!(w.step(true, false, Some(90.0), 30_000), Some(Action::BoostOff));
    // Off now: still warm, so it stays off.
    assert_eq!(w.step(true, true, Some(84.0), 33_000), None);
    assert_eq!(w.step(true, true, Some(75.0), 36_000), None);
    assert_eq!(w.step(true, true, Some(70.0), 150_000), None);
    assert_eq!(w.step(true, true, Some(72.0), 156_000), Some(Action::BoostOn));
  }

  #[test]
  fn a_dip_restarts_the_timers() {
    let mut w = Watch::default();
    assert_eq!(w.step(true, false, Some(95.0), 0), None);
    assert_eq!(w.step(true, false, Some(89.0), 20_000), None);
    assert_eq!(w.step(true, false, Some(95.0), 40_000), None);
    assert_eq!(w.step(true, false, Some(95.0), 69_000), None);
    assert_eq!(w.step(true, false, Some(95.0), 70_000), Some(Action::BoostOff));
    assert_eq!(w.step(true, true, Some(70.0), 80_000), None);
    assert_eq!(w.step(true, true, Some(80.0), 150_000), None);
    assert_eq!(w.step(true, true, Some(70.0), 200_000), None);
    assert_eq!(w.step(true, true, Some(70.0), 319_000), None);
    assert_eq!(w.step(true, true, Some(70.0), 320_000), Some(Action::BoostOn));
  }

  #[test]
  fn turning_it_off_or_losing_the_sensors_brings_boost_back() {
    let mut w = Watch::default();
    assert_eq!(w.step(false, true, Some(95.0), 0), Some(Action::BoostOn));
    assert_eq!(w.step(false, false, Some(95.0), 60_000), None);
    let mut w = Watch::default();
    assert_eq!(w.step(true, true, None, 0), None);
    assert_eq!(w.step(true, true, None, 120_000), Some(Action::BoostOn));
    assert_eq!(w.step(true, false, None, 0), None);
  }

  #[test]
  fn reads_powercfg_output() {
    assert_eq!(active_scheme("Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)").as_deref(), Some("381b4222-f694-41f0-9685-ff5bb260df2e"));
    let q = "      Possible Setting Index: 006\r\n      Possible Setting Friendly Name: Efficient Aggressive At Guaranteed\r\n    Current AC Power Setting Index: 0x00000002\r\n    Current DC Power Setting Index: 0x00000001\r\n";
    assert_eq!(boost_values(q), Some((2, 1)));
    assert_eq!(boost_values("nothing"), None);
    assert!(valid_scheme("381b4222-f694-41f0-9685-ff5bb260df2e"));
    assert!(!valid_scheme("381b4222-f694-41f0-9685-ff5bb260df2e /x") && !valid_scheme("/hibernate") && !valid_scheme(""));
  }
}
