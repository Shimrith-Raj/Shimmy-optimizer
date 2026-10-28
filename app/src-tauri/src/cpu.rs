//! The CPU section: load per thread (sysinfo), temperature, real clocks and power (LibreHardwareMonitor sidecar),
//! fans (ASUS ATK), a plain-language status with throttling detection. The GPU is in gpu.rs.
use crate::sensors::{value, Reading};
use serde::Serialize;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Core {
  pub clock_mhz: Option<f64>,
  pub power_w: Option<f64>,
  pub load_pct: f32,
}

#[derive(Serialize, Debug)]
pub struct Fan {
  pub name: String,
  pub rpm: u32,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Status {
  pub level: &'static str,
  pub text: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CpuView {
  pub name: String,
  pub threads: usize,
  pub base_mhz: Option<f64>,
  pub load_pct: f32,
  pub temp_c: Option<f64>,
  pub package_w: Option<f64>,
  pub avg_clock_mhz: Option<f64>,
  pub cores: Vec<Core>,
  pub fans: Vec<Fan>,
  pub sensors_live: bool,
  pub sensors_error: Option<String>,
  pub status: Status,
}

pub fn status(temp: Option<f64>, load: f32, avg_clock: Option<f64>, base: Option<f64>) -> Status {
  let throttled = match (avg_clock, base) {
    (Some(c), Some(b)) if load >= 60.0 && c < b * 0.8 => Some((c / 1000.0, b / 1000.0)),
    _ => None,
  };
  let s = |level, text: String| Status { level, text };
  match (temp, throttled) {
    (Some(t), Some((c, _))) if t >= 95.0 => s("bad", format!("Throttling: {c:.1} GHz under load at {t:.0} °C. It is too hot to run at full speed.")),
    (Some(t), None) if t >= 95.0 => s("bad", format!("Very hot at {t:.0} °C. The processor will slow itself down to cool off.")),
    (Some(t), _) if t >= 85.0 => s("warn", format!("Warm: {t:.0} °C at {load:.0}% load.")),
    (_, Some((c, b))) => s("warn", format!("Running at {c:.1} GHz while busy, below its rated {b:.1} GHz. A battery-saving power mode does this.")),
    _ if load >= 90.0 => s("warn", format!("Working flat out at {load:.0}% load.")),
    (Some(t), _) if load < 15.0 => s("good", format!("Idle and cool at {t:.0} °C.")),
    (Some(t), _) => s("good", format!("Running cool at {t:.0} °C.")),
    (None, _) => s("good", format!("Running normally at {load:.0}% load.")),
  }
}

/// The rated clock Windows records for the processor (HKLM ... CentralProcessor\0, "~MHz").
pub fn base_mhz() -> Option<f64> {
  use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
  RegKey::predef(HKEY_LOCAL_MACHINE)
    .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
    .and_then(|k| k.get_value::<u32, _>("~MHz"))
    .ok()
    .map(f64::from)
}

pub fn build(name: String, thread_loads: &[f32], load_pct: f32, base: Option<f64>, rows: Option<&[Reading]>, fans: Vec<Fan>, sensors_error: Option<String>) -> CpuView {
  let rows = rows.unwrap_or(&[]);
  let n_cores = (thread_loads.len() / 2).max(1);
  // Windows lists SMT siblings next to each other, so threads 2k and 2k+1 share core k.
  let cores: Vec<Core> = (0..n_cores)
    .map(|k| {
      let pair: Vec<f32> = thread_loads.iter().skip(k * 2).take(2).copied().collect();
      Core {
        clock_mhz: value(rows, "Cpu", "Clock", &format!("Core #{}", k + 1)),
        power_w: value(rows, "Cpu", "Power", &format!("Core #{} (SMU)", k + 1)),
        load_pct: pair.iter().sum::<f32>() / pair.len().max(1) as f32,
      }
    })
    .collect();
  let clocks: Vec<f64> = cores.iter().filter_map(|c| c.clock_mhz).collect();
  let avg_clock = (!clocks.is_empty()).then(|| clocks.iter().sum::<f64>() / clocks.len() as f64);
  let temp = value(rows, "Cpu", "Temperature", "Core (Tctl/Tdie)").or_else(|| value(rows, "Cpu", "Temperature", "Core (Tctl)"));
  CpuView {
    name,
    threads: thread_loads.len(),
    base_mhz: base,
    load_pct,
    temp_c: temp,
    package_w: value(rows, "Cpu", "Power", "Package"),
    avg_clock_mhz: avg_clock,
    status: status(temp, load_pct, avg_clock, base),
    cores,
    fans,
    sensors_live: !rows.is_empty(),
    sensors_error,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn phase0_rows() -> Vec<Reading> {
    let text = include_str!("../tests/fixtures/sensors.json").trim_start_matches('\u{feff}');
    serde_json::from_str(text).unwrap()
  }

  #[test]
  fn reads_the_phase0_sensor_dump() {
    let rows = phase0_rows();
    let v = build("Ryzen".into(), &[10.0; 16], 10.0, Some(3200.0), Some(&rows), vec![], None);
    assert_eq!(v.temp_c, Some(61.25));
    assert_eq!(v.package_w, Some(4.8));
    assert_eq!(v.cores.len(), 8);
    assert_eq!(v.cores[0].clock_mhz, Some(1373.0));
    assert!(v.sensors_live);
    assert_eq!(v.status.level, "good");
  }

  #[test]
  fn works_without_sensors() {
    let v = build("Ryzen".into(), &[50.0, 30.0], 40.0, None, None, vec![], Some("not admin".into()));
    assert_eq!(v.temp_c, None);
    assert_eq!(v.cores.len(), 1);
    assert_eq!(v.cores[0].load_pct, 40.0);
    assert_eq!(v.status, Status { level: "good", text: "Running normally at 40% load.".into() });
  }

  #[test]
  fn status_rules() {
    assert_eq!(status(Some(97.0), 90.0, Some(2000.0), Some(3200.0)).level, "bad");
    assert!(status(Some(97.0), 90.0, Some(2000.0), Some(3200.0)).text.starts_with("Throttling"));
    assert_eq!(status(Some(88.0), 50.0, None, None).level, "warn");
    assert!(status(Some(60.0), 80.0, Some(1800.0), Some(3200.0)).text.contains("power mode"));
    assert_eq!(status(Some(50.0), 5.0, Some(1400.0), Some(3200.0)).text, "Idle and cool at 50 °C.");
  }
}
