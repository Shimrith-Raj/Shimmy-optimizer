//! Automation: alerts, automatic memory clean and the weekly clean. The decisions are pure functions so they can be
//! unit-tested; lib.rs feeds them readings from the sampler and acts on what they return.
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Alerts {
  pub enabled: bool,
  /// Memory in use, %, held for a minute.
  pub ram_pct: u32,
  /// Processor temperature, °C, held for 30 seconds.
  pub cpu_temp_c: u32,
  /// Free space on C:, GB.
  pub disk_free_gb: u32,
  /// Battery charge, %, while on battery.
  pub battery_pct: u32,
}

impl Default for Alerts {
  fn default() -> Self {
    Alerts { enabled: true, ram_pct: 90, cpu_temp_c: 90, disk_free_gb: 10, battery_pct: 15 }
  }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoClean {
  pub enabled: bool,
  pub at_pct: u32,
  /// How long memory must stay above `at_pct` first.
  pub for_min: u32,
  /// "trim" asks apps to release memory only; "clean" also closes idle background apps marked safe (Clean now).
  pub level: String,
}

impl Default for AutoClean {
  fn default() -> Self {
    AutoClean { enabled: false, at_pct: 85, for_min: 5, level: "trim".into() }
  }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Weekly {
  pub enabled: bool,
  /// 0 = Sunday … 6 = Saturday.
  pub day: u32,
  pub hour: u32,
  /// Clean category ids (clean.rs).
  pub cats: Vec<String>,
  /// When the weekly clean last ran (ms). Kept by Shimmy, not editable from the page.
  pub last_run: u64,
}

impl Default for Weekly {
  fn default() -> Self {
    let cats = ["user-temp", "win-temp", "thumbs", "dumps", "delivery", "wu", "brave", "chrome", "edge"];
    Weekly { enabled: false, day: 0, hour: 12, cats: cats.iter().map(|c| c.to_string()).collect(), last_run: 0 }
  }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct FinderPrefs {
  /// Folders searched besides the five standard ones.
  pub extra_roots: Vec<String>,
  /// Standard folders turned off (by path).
  pub skip_roots: Vec<String>,
  /// Files whose path contains any of these (ignoring case) are never offered.
  pub exclusions: Vec<String>,
  pub large_mb: u64,
}

impl Default for FinderPrefs {
  fn default() -> Self {
    FinderPrefs { extra_roots: Vec::new(), skip_roots: Vec::new(), exclusions: Vec::new(), large_mb: 500 }
  }
}

/// Brings values from the page into sensible ranges.
pub fn clamp(a: &mut Alerts, c: &mut AutoClean, w: &mut Weekly, f: &mut FinderPrefs) {
  a.ram_pct = a.ram_pct.clamp(50, 99);
  a.cpu_temp_c = a.cpu_temp_c.clamp(60, 105);
  a.disk_free_gb = a.disk_free_gb.clamp(1, 200);
  a.battery_pct = a.battery_pct.clamp(5, 50);
  c.at_pct = c.at_pct.clamp(50, 98);
  c.for_min = c.for_min.clamp(1, 60);
  if c.level != "clean" {
    c.level = "trim".into();
  }
  w.day = w.day.min(6);
  w.hour = w.hour.min(23);
  w.cats.retain(|id| crate::clean::is_category(id));
  w.cats.dedup();
  f.large_mb = f.large_mb.clamp(50, 100_000);
  for list in [&mut f.extra_roots, &mut f.skip_roots, &mut f.exclusions] {
    list.iter_mut().for_each(|s| *s = s.trim().to_string());
    list.retain(|s| !s.is_empty());
    list.dedup();
    list.truncate(50);
  }
}

/// Fires once when a condition has held for `hold_ms`; again only after it clears, or after `repeat_ms` if it never does.
#[derive(Default)]
pub struct Rule {
  since: Option<u64>,
  last: Option<u64>,
}

impl Rule {
  pub fn step(&mut self, bad: bool, now: u64, hold_ms: u64, repeat_ms: u64) -> bool {
    if !bad {
      self.since = None;
      self.last = None;
      return false;
    }
    let since = *self.since.get_or_insert(now);
    if now.saturating_sub(since) < hold_ms {
      return false;
    }
    if self.last.is_some_and(|l| now.saturating_sub(l) < repeat_ms) {
      return false;
    }
    self.last = Some(now);
    true
  }
}

/// The alert and auto-clean rules, held by the sampler thread.
#[derive(Default)]
pub struct Watch {
  pub ram: Rule,
  pub cpu: Rule,
  pub disk: Rule,
  pub battery: Rule,
  pub auto_clean: Rule,
}

/// Local time parts: day of week (0 = Sunday), hour, minute, second, millisecond.
pub struct Local {
  pub dow: u32,
  pub hour: u32,
  pub min: u32,
  pub sec: u32,
  pub ms: u32,
}

pub fn local_now() -> Local {
  let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
  Local { dow: t.wDayOfWeek as u32, hour: t.wHour as u32, min: t.wMinute as u32, sec: t.wSecond as u32, ms: t.wMilliseconds as u32 }
}

/// The most recent weekly slot (`day` at `hour`:00) at or before now, in ms since the epoch.
pub fn last_slot(now_ms: u64, l: &Local, day: u32, hour: u32) -> u64 {
  let mut days_back = (l.dow + 7 - day) % 7;
  if days_back == 0 && l.hour < hour {
    days_back = 7;
  }
  let into_today = ((l.hour * 3600 + l.min * 60 + l.sec) * 1000 + l.ms) as i64;
  let back = days_back as i64 * 86_400_000 + into_today - hour as i64 * 3_600_000;
  (now_ms as i64 - back).max(0) as u64
}

/// True when a weekly slot has passed since the last run, so a PC that was off at that time catches up later.
pub fn weekly_due(w: &Weekly, now_ms: u64, l: &Local) -> bool {
  w.enabled && w.last_run < last_slot(now_ms, l, w.day, w.hour)
}

/// When the next weekly clean will run, in ms since the epoch.
pub fn next_slot(now_ms: u64, l: &Local, day: u32, hour: u32) -> u64 {
  last_slot(now_ms, l, day, hour) + 7 * 86_400_000
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rule_holds_then_fires_once_until_cleared() {
    let mut r = Rule::default();
    assert!(!r.step(true, 0, 60_000, 1_800_000));
    assert!(!r.step(true, 59_000, 60_000, 1_800_000));
    assert!(r.step(true, 60_000, 60_000, 1_800_000));
    assert!(!r.step(true, 120_000, 60_000, 1_800_000));
    assert!(r.step(true, 60_000 + 1_800_000, 60_000, 1_800_000), "repeats after the cooldown");
    assert!(!r.step(false, 2_000_000, 60_000, 1_800_000));
    assert!(!r.step(true, 2_010_000, 60_000, 1_800_000), "hold starts again after clearing");
    assert!(r.step(true, 2_070_000, 60_000, 1_800_000));
  }

  #[test]
  fn weekly_slots() {
    // Wednesday 15:30:00.000; slot Sunday 12:00 was 3 days and 3.5 hours ago.
    let now = 1_000_000_000_000;
    let wed = Local { dow: 3, hour: 15, min: 30, sec: 0, ms: 0 };
    assert_eq!(last_slot(now, &wed, 0, 12), now - (3 * 86_400_000 + 3 * 3_600_000 + 1_800_000));
    // Same day, slot later today: last week's.
    assert_eq!(last_slot(now, &wed, 3, 18), now - (7 * 86_400_000 - 2 * 3_600_000 - 1_800_000));
    // Same day, slot earlier today.
    assert_eq!(last_slot(now, &wed, 3, 9), now - (6 * 3_600_000 + 1_800_000));
    let mut w = Weekly { enabled: true, day: 3, hour: 9, ..Default::default() };
    assert!(weekly_due(&w, now, &wed));
    w.last_run = now - 3_600_000;
    assert!(!weekly_due(&w, now, &wed), "already ran after this morning's slot");
    w.enabled = false;
    w.last_run = 0;
    assert!(!weekly_due(&w, now, &wed));
  }

  #[test]
  fn clamping() {
    let (mut a, mut c, mut w, mut f) = (Alerts { ram_pct: 5, ..Default::default() }, AutoClean { level: "nuke".into(), for_min: 0, ..Default::default() },
      Weekly { day: 9, cats: vec!["wu".into(), "rm -rf".into()], ..Default::default() }, FinderPrefs { exclusions: vec!["  ".into(), " Taxes ".into()], ..Default::default() });
    clamp(&mut a, &mut c, &mut w, &mut f);
    assert_eq!((a.ram_pct, c.level.as_str(), c.for_min, w.day), (50, "trim", 1, 6));
    assert_eq!(w.cats, vec!["wu".to_string()]);
    assert_eq!(f.exclusions, vec!["Taxes".to_string()]);
  }
}
