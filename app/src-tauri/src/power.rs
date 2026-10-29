//! Power mode follows the charger: Windows' power mode (the slider in Settings > Power, an "overlay" on the power plan)
//! switched when the charger is plugged in or pulled out. Only on that change, so a mode picked by hand in Windows stays
//! until the next one. The old mode is saved first and put back on Exit and when this is turned off.
//! The overlay functions are exported by powrprof.dll but not in the `windows` crate, so they are linked here. Tested on
//! the Vivobook (PLAN_premium_features.md, P1 Step 1): no elevation needed, and heat protection's `powercfg /setactive`
//! leaves the overlay alone.
use serde::{Deserialize, Serialize};
use windows::core::GUID;

#[link(name = "powrprof", kind = "raw-dylib")]
unsafe extern "system" {
  fn PowerGetEffectiveOverlayScheme(overlay: *mut GUID) -> u32;
  fn PowerSetActiveOverlayScheme(overlay: GUID) -> u32;
}

const EFFICIENCY: GUID = GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a);
const PERFORMANCE: GUID = GUID::from_u128(0xded574b5_45a0_4f42_8737_46345c09c238);
/// Balanced is no overlay at all.
const BALANCED: GUID = GUID::zeroed();

pub const MODES: [&str; 3] = ["efficiency", "balanced", "performance"];

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct PowerPrefs {
  pub follow: bool,
  /// One of `MODES`, while the charger is plugged in.
  pub plugged: String,
  /// One of `MODES`, on battery.
  pub battery: String,
  /// The mode before Shimmy first changed it (a GUID as hex), put back on Exit and when this is turned off. Kept by
  /// Shimmy.
  pub saved: Option<String>,
}

impl Default for PowerPrefs {
  fn default() -> Self {
    PowerPrefs { follow: false, plugged: "performance".into(), battery: "balanced".into(), saved: None }
  }
}

/// Unknown mode names from the page fall back to the defaults.
pub fn clamp(p: &mut PowerPrefs) {
  let d = PowerPrefs::default();
  if !MODES.contains(&p.plugged.as_str()) {
    p.plugged = d.plugged;
  }
  if !MODES.contains(&p.battery.as_str()) {
    p.battery = d.battery;
  }
}

pub fn guid_of(mode: &str) -> GUID {
  match mode {
    "efficiency" => EFFICIENCY,
    "performance" => PERFORMANCE,
    _ => BALANCED,
  }
}

/// The name Windows shows for a mode. A laptop maker can add its own overlays; those are "another mode".
pub fn label(g: GUID) -> &'static str {
  match g {
    EFFICIENCY => "Best power efficiency",
    PERFORMANCE => "Best performance",
    BALANCED => "Balanced",
    _ => "another power mode",
  }
}

pub fn to_text(g: GUID) -> String {
  format!("{:032x}", g.to_u128())
}

pub fn from_text(t: &str) -> Option<GUID> {
  u128::from_str_radix(t, 16).ok().map(GUID::from_u128)
}

pub fn current() -> Option<GUID> {
  let mut g = GUID::zeroed();
  (unsafe { PowerGetEffectiveOverlayScheme(&mut g) } == 0).then_some(g)
}

pub fn set(g: GUID) -> Result<(), String> {
  match unsafe { PowerSetActiveOverlayScheme(g) } {
    0 => Ok(()),
    e => Err(format!("Windows refused the power mode change (error {e}).")),
  }
}

/// Whether to switch now, and for which side: Some(true) for the charger's mode, Some(false) for battery's. Only when
/// the charger state differs from the last one acted on (`last`), so a mode picked by hand in Windows is left alone.
/// While heat protection has boost off it waits, and switches once boost is back.
pub fn decide(follow: bool, boost_off: bool, plugged: bool, last: Option<bool>) -> Option<bool> {
  (follow && !boost_off && last != Some(plugged)).then_some(plugged)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn switches_only_when_the_charger_changes() {
    assert_eq!(decide(true, false, true, None), Some(true), "first tick after turning it on");
    assert_eq!(decide(true, false, true, Some(true)), None, "still plugged in: leave a hand-picked mode alone");
    assert_eq!(decide(true, false, false, Some(true)), Some(false), "unplugged");
    assert_eq!(decide(false, false, false, Some(true)), None, "off");
    assert_eq!(decide(true, true, false, Some(true)), None, "waits while heat protection has boost off");
  }

  #[test]
  fn modes_round_trip() {
    for m in MODES {
      assert_eq!(from_text(&to_text(guid_of(m))), Some(guid_of(m)));
    }
    assert_eq!(label(guid_of("performance")), "Best performance");
    assert_eq!(label(guid_of("nonsense")), "Balanced");
    let mut p = PowerPrefs { plugged: "turbo".into(), battery: "efficiency".into(), ..Default::default() };
    clamp(&mut p);
    assert_eq!((p.plugged.as_str(), p.battery.as_str()), ("performance", "efficiency"));
  }
}
