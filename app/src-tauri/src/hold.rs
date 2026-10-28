//! "Run from the charger": while plugged in, the battery neither charges nor drains, so the charger powers the laptop and
//! charge cycles are saved. Done with the ASUS firmware charge limit: on this laptop a limit at or below the current
//! charge makes the battery rest (it never drains down to the limit) and any whole % is accepted (PLAN.md, Phase 7).
//! The decisions are pure functions; lib.rs feeds them readings every 3 s and carries out what they return.
use serde::{Deserialize, Serialize};

/// Never hold below this: under it the battery charges up to it first.
pub const FLOOR: u32 = 40;
/// The firmware takes about a minute to act on a new limit, so charging above the hold is tolerated this long.
const GRACE_MS: u64 = 3 * 60_000;
/// Charging or draining slower than this is noise or self-discharge.
const MOVING_W: f64 = 1.0;
const DRAIN_W: f64 = 3.0;

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct PluggedMode {
  pub on: bool,
  pub hold_pct: u32,
}

/// The level to hold for a charge reading: the whole % below it (so nothing drains), never under the floor.
pub fn hold_for(charge_pct: f64) -> u32 {
  (charge_pct.floor().max(0.0) as u32).clamp(FLOOR, 100)
}

/// What the firmware limit should be: the hold level while the mode is on, otherwise the 60/80/100 choice.
pub fn effective_limit(mode: &PluggedMode, charge_limit: Option<u32>) -> Option<u32> {
  if mode.on { Some(mode.hold_pct) } else { charge_limit }
}

pub struct Reading {
  pub plugged: bool,
  pub charge_pct: f64,
  /// Watts; positive while charging, negative while discharging. None when the sensors are not delivering.
  pub rate_w: Option<f64>,
}

#[derive(Debug, PartialEq)]
pub enum Action {
  /// Plugged in (or Shimmy started plugged in): hold at this new level.
  Hold(u32),
  /// Charging above the hold for a while: write the limit again.
  Reapply,
  /// Rewriting did not stop it: something (MyASUS) keeps changing the limit.
  WarnOverride,
  /// The battery is helping the charger under heavy load, this many watts.
  WarnDrain(f64),
}

#[derive(Default)]
pub struct Watch {
  was_plugged: Option<bool>,
  over_since: Option<u64>,
  drain_since: Option<u64>,
  reapplied: u32,
  pub override_warned: bool,
  drain_warned: bool,
}

impl Watch {
  /// The plug-in hold could not be written: treat the next reading as a fresh plug-in so it is tried again.
  pub fn retry_plug_in(&mut self) {
    self.was_plugged = None;
  }

  pub fn step(&mut self, mode: &PluggedMode, r: &Reading, now: u64) -> Vec<Action> {
    let plugged_in_now = r.plugged && self.was_plugged != Some(true);
    self.was_plugged = Some(r.plugged);
    let mut out = Vec::new();
    if !mode.on || !r.plugged {
      self.over_since = None;
      self.drain_since = None;
      return out;
    }
    if plugged_in_now {
      // A new plug-in session: hold where the battery is now, and give the firmware time to act.
      self.over_since = None;
      self.drain_since = None;
      self.reapplied = 0;
      self.drain_warned = false;
      let hold = hold_for(r.charge_pct);
      if hold != mode.hold_pct {
        out.push(Action::Hold(hold));
      }
      return out;
    }
    let Some(rate) = r.rate_w else { return out };

    // Charging although the battery is already above the hold level.
    if rate > MOVING_W && r.charge_pct > mode.hold_pct as f64 + 0.5 {
      let since = *self.over_since.get_or_insert(now);
      if now.saturating_sub(since) >= GRACE_MS {
        self.over_since = Some(now);
        self.reapplied += 1;
        out.push(Action::Reapply);
        if self.reapplied >= 2 && !self.override_warned {
          self.override_warned = true;
          out.push(Action::WarnOverride);
        }
      }
    } else {
      self.over_since = None;
    }

    // Draining while plugged in: the charger cannot keep up with this load.
    if rate < -DRAIN_W {
      let since = *self.drain_since.get_or_insert(now);
      if now.saturating_sub(since) >= 30_000 && !self.drain_warned {
        self.drain_warned = true;
        out.push(Action::WarnDrain(-rate));
      }
    } else {
      self.drain_since = None;
    }
    out
  }
}

/// One plain sentence for the Battery page.
pub fn describe(mode: &PluggedMode, r: &Reading) -> String {
  let h = mode.hold_pct;
  if !mode.on {
    return "Off. When on, the battery stops charging where it is while you are plugged in, and the charger powers the laptop. That saves charge cycles.".into();
  }
  if !r.plugged {
    return format!("On battery now. When you plug back in, Shimmy holds the battery at whatever level it is then (never below {FLOOR}%).");
  }
  let rate = r.rate_w.unwrap_or(0.0);
  if rate < -DRAIN_W {
    return format!("Holding at {h}%, but the battery is helping: {:.0} W are coming from it because the charger cannot keep up with this load.", -rate);
  }
  if rate > MOVING_W && r.charge_pct < h as f64 - 0.5 {
    return format!("Charging up to {h}%, then the battery rests and the charger powers the laptop.");
  }
  if rate > MOVING_W {
    return format!("Holding at {h}%. The firmware can take a minute to stop charging after a change.");
  }
  format!("Holding at {h}%. The charger is powering the laptop and the battery is resting.")
}

/// MyASUS Battery Care's own limit, from its settings. MyASUS can write it back to the firmware (at start-up, for one).
pub fn myasus_limit() -> Option<u32> {
  use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
  let key = RegKey::predef(HKEY_LOCAL_MACHINE)
    .open_subkey(r"SOFTWARE\ASUS\ASUS System Control Interface\AsusOptimization\ASUS Keyboard Hotkeys")
    .ok()?;
  key.get_value::<u32, _>("ChargingRate").ok().filter(|v| (20..=100).contains(v))
}

#[cfg(test)]
mod tests {
  use super::*;

  const MIN: u64 = 60_000;
  fn on(h: u32) -> PluggedMode {
    PluggedMode { on: true, hold_pct: h }
  }
  fn rd(plugged: bool, charge: f64, rate: f64) -> Reading {
    Reading { plugged, charge_pct: charge, rate_w: Some(rate) }
  }

  #[test]
  fn hold_levels() {
    assert_eq!(hold_for(60.6), 60, "rounds down so nothing drains");
    assert_eq!(hold_for(15.0), FLOOR);
    assert_eq!(hold_for(100.0), 100);
    assert_eq!(effective_limit(&on(55), Some(80)), Some(55));
    assert_eq!(effective_limit(&PluggedMode { on: false, hold_pct: 55 }, Some(80)), Some(80));
  }

  #[test]
  fn replug_holds_at_the_new_level() {
    let mut w = Watch::default();
    assert_eq!(w.step(&on(70), &rd(true, 70.2, 0.0), 0), vec![], "started plugged in at the hold level");
    assert_eq!(w.step(&on(70), &rd(false, 60.0, -9.0), MIN), vec![]);
    assert_eq!(w.step(&on(70), &rd(true, 45.4, 0.0), 2 * MIN), vec![Action::Hold(45)]);
    // Below the floor it charges to the floor first.
    w.step(&on(45), &rd(false, 30.0, -9.0), 3 * MIN);
    assert_eq!(w.step(&on(45), &rd(true, 30.0, 0.0), 4 * MIN), vec![Action::Hold(40)]);
    // Starting Shimmy plugged in after an unplugged session counts as a plug-in.
    let mut fresh = Watch::default();
    assert_eq!(fresh.step(&on(70), &rd(true, 52.0, 0.0), 0), vec![Action::Hold(52)]);
  }

  #[test]
  fn reapplies_then_warns_about_an_override() {
    let mut w = Watch::default();
    let m = on(60);
    w.step(&m, &rd(true, 60.0, 0.0), 0);
    assert_eq!(w.step(&m, &rd(true, 61.0, 40.0), MIN), vec![]);
    assert_eq!(w.step(&m, &rd(true, 62.0, 40.0), 2 * MIN), vec![], "within the firmware's grace period");
    assert_eq!(w.step(&m, &rd(true, 64.0, 40.0), 4 * MIN), vec![Action::Reapply]);
    assert_eq!(w.step(&m, &rd(true, 68.0, 40.0), 7 * MIN), vec![Action::Reapply, Action::WarnOverride]);
    assert_eq!(w.step(&m, &rd(true, 72.0, 40.0), 10 * MIN), vec![Action::Reapply], "warns only once");
    // Charging up to a hold level above the charge is expected.
    let mut up = Watch::default();
    up.step(&on(40), &rd(true, 30.0, 0.0), 0);
    assert_eq!(up.step(&on(40), &rd(true, 33.0, 45.0), 10 * MIN), vec![]);
  }

  #[test]
  fn warns_once_per_plug_in_about_heavy_load() {
    let mut w = Watch::default();
    let m = on(60);
    w.step(&m, &rd(true, 60.0, 0.0), 0);
    assert_eq!(w.step(&m, &rd(true, 59.8, -12.0), 10_000), vec![]);
    assert_eq!(w.step(&m, &rd(true, 59.5, -12.0), 40_000), vec![Action::WarnDrain(12.0)]);
    assert_eq!(w.step(&m, &rd(true, 59.0, -12.0), 80_000), vec![]);
  }

  #[test]
  fn sentences() {
    assert!(describe(&on(60), &rd(true, 60.0, 0.0)).contains("battery is resting"));
    assert!(describe(&on(40), &rd(true, 32.0, 40.0)).starts_with("Charging up to 40%"));
    assert!(describe(&on(60), &rd(true, 59.0, -15.0)).contains("15 W"));
    assert!(describe(&on(60), &rd(false, 50.0, -8.0)).starts_with("On battery"));
    assert!(describe(&PluggedMode::default(), &rd(true, 50.0, 0.0)).starts_with("Off."));
  }
}
