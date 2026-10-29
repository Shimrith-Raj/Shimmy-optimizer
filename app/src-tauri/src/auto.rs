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
  /// Junk the Clean page would clean by default (its ticked categories), GB, checked hourly. 0 is off.
  pub junk_gb: u32,
}

impl Default for Alerts {
  fn default() -> Self {
    Alerts { enabled: true, ram_pct: 90, cpu_temp_c: 90, disk_free_gb: 10, battery_pct: 15, junk_gb: 5 }
  }
}

/// True when the junk alert is on and `bytes` of junk reach its threshold.
pub fn junk_over(bytes: u64, junk_gb: u32) -> bool {
  junk_gb > 0 && bytes >= junk_gb as u64 * 1_073_741_824
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

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdatePrefs {
  /// Look for a new version once a day. It reads GitHub's public release info and uploads nothing.
  pub auto_check: bool,
  /// Kept by Shimmy, not editable from the page: when the last daily check ran (ms), the newest version it saw, and
  /// the version it already announced.
  pub last_check: u64,
  pub latest_seen: String,
  pub last_notified: String,
}

impl Default for UpdatePrefs {
  fn default() -> Self {
    UpdatePrefs { auto_check: true, last_check: 0, latest_seen: String::new(), last_notified: String::new() }
  }
}

const DAY_MS: u64 = 86_400_000;

/// True when the daily check is on and none has run in the last 24 hours.
pub fn update_due(u: &UpdatePrefs, now_ms: u64) -> bool {
  u.auto_check && now_ms.saturating_sub(u.last_check) >= DAY_MS
}

/// When a daily check fails (offline, say), the stamp that makes the next one due in an hour instead of tomorrow.
pub fn update_retry_stamp(now_ms: u64) -> u64 {
  now_ms.saturating_sub(DAY_MS - 3_600_000)
}

/// One notification per new version: only for a version newer than this one that was not announced already.
pub fn update_notify_due(u: &UpdatePrefs, latest: &str, newer: bool) -> bool {
  newer && u.last_notified != latest
}

/// Brings values from the page into sensible ranges.
pub fn clamp(a: &mut Alerts, c: &mut AutoClean, w: &mut Weekly, f: &mut FinderPrefs, u: &mut UpdatePrefs) {
  for v in [&mut u.latest_seen, &mut u.last_notified] {
    v.retain(|ch| ch.is_ascii_digit() || ch == '.');
    v.truncate(32);
  }
  a.ram_pct = a.ram_pct.clamp(50, 99);
  a.cpu_temp_c = a.cpu_temp_c.clamp(60, 105);
  a.disk_free_gb = a.disk_free_gb.clamp(1, 200);
  a.battery_pct = a.battery_pct.clamp(5, 50);
  a.junk_gb = a.junk_gb.min(200);
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
  /// Stepped hourly, from the junk scan's own thread.
  pub junk: Rule,
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
  fn daily_update_check() {
    let day = 86_400_000;
    let mut u = UpdatePrefs::default();
    assert!(u.auto_check, "on by default");
    assert!(update_due(&u, day), "never checked");
    u.last_check = 5 * day;
    assert!(!update_due(&u, 5 * day + day - 1));
    assert!(update_due(&u, 6 * day));
    u.auto_check = false;
    assert!(!update_due(&u, 100 * day), "off means never");
    u.auto_check = true;
    u.last_check = update_retry_stamp(10 * day);
    assert!(!update_due(&u, 10 * day + 3_599_000));
    assert!(update_due(&u, 10 * day + 3_600_000), "a failed check is retried in an hour");
    assert_eq!(update_retry_stamp(1000), 0, "no underflow");
    u.last_notified = "0.5.1".into();
    assert!(!update_notify_due(&u, "0.5.1", true), "once per version");
    assert!(update_notify_due(&u, "0.5.2", true));
    assert!(!update_notify_due(&u, "0.5.2", false), "never for a version that is not newer");
  }

  #[test]
  fn junk_alert() {
    let gb = 1_073_741_824;
    assert!(!junk_over(10 * gb, 0), "0 is off");
    assert!(!junk_over(5 * gb - 1, 5));
    assert!(junk_over(5 * gb, 5));
    // Hourly checks: once, then at most once a day while it stays above.
    let (mut r, hour, day) = (Rule::default(), 3_600_000, 86_400_000);
    assert!(r.step(true, hour, 0, day));
    assert!(!r.step(true, 2 * hour, 0, day));
    assert!(r.step(true, hour + day, 0, day));
  }

  #[test]
  fn clamping() {
    let (mut a, mut c, mut w, mut f) = (Alerts { ram_pct: 5, ..Default::default() }, AutoClean { level: "nuke".into(), for_min: 0, ..Default::default() },
      Weekly { day: 9, cats: vec!["wu".into(), "rm -rf".into()], ..Default::default() }, FinderPrefs { exclusions: vec!["  ".into(), " Taxes ".into()], ..Default::default() });
    let mut u = UpdatePrefs { latest_seen: "v0.5.1<b>".into(), last_notified: "9".repeat(40), ..Default::default() };
    clamp(&mut a, &mut c, &mut w, &mut f, &mut u);
    assert_eq!((u.latest_seen.as_str(), u.last_notified.len()), ("0.5.1", 32));
    assert_eq!((a.ram_pct, c.level.as_str(), c.for_min, w.day), (50, "trim", 1, 6));
    assert_eq!(w.cats, vec!["wu".to_string()]);
    assert_eq!(f.exclusions, vec!["Taxes".to_string()]);
  }
}
