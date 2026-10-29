//! Cooling check: a short full-load test the user starts from the Processor page. Every logical processor is kept
//! busy (at the lowest thread priority, so the PC stays usable) for 60 to 120 s. It stops early at `STOP_C`, when the
//! temperature reading is lost, or on Stop. The verdict is a pure function of the samples, so it is unit-tested.
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST};

/// The check stops at this temperature, whatever is left of its time.
pub const STOP_C: f64 = 95.0;
/// It also stops if the temperature cannot be read for this long, since it could not stop at `STOP_C` then.
pub const NO_TEMP_MS: u64 = 10_000;

pub fn clamp_secs(s: u32) -> u32 {
  s.clamp(60, 120)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
  pub t_ms: u64,
  pub temp: Option<f64>,
  pub clock: Option<f64>,
  pub watts: Option<f64>,
  pub fan: Option<u32>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
  pub level: &'static str,
  pub text: String,
  pub seconds: u64,
  /// "time", "hot", "stopped" or "no-temp".
  pub stop: &'static str,
  pub start_c: Option<f64>,
  pub peak_c: Option<f64>,
  /// Average effective clock over the second half, when the processor has settled.
  pub clock_mhz: Option<f64>,
  pub base_mhz: Option<f64>,
  pub peak_w: Option<f64>,
  pub peak_fan: Option<u32>,
  pub throttled: bool,
  /// Heat protection turned boost off during the check.
  pub boost_off: bool,
}

/// Why to stop now, if at all.
pub fn stop_reason(elapsed_ms: u64, limit_ms: u64, temp: Option<f64>, no_temp_ms: u64, stopped: bool) -> Option<&'static str> {
  if stopped {
    Some("stopped")
  } else if temp.is_some_and(|t| t >= STOP_C) {
    Some("hot")
  } else if no_temp_ms >= NO_TEMP_MS {
    Some("no-temp")
  } else if elapsed_ms >= limit_ms {
    Some("time")
  } else {
    None
  }
}

fn max_f(it: impl Iterator<Item = f64>) -> Option<f64> {
  it.fold(None, |m, v| Some(m.map_or(v, |m: f64| m.max(v))))
}

pub fn verdict(samples: &[Sample], stop: &'static str, base: Option<f64>, boost_off: bool) -> Verdict {
  let seconds = samples.last().map_or(0, |s| s.t_ms / 1000);
  let peak_c = max_f(samples.iter().filter_map(|s| s.temp));
  let half = samples.last().map_or(0, |s| s.t_ms / 2);
  let late: Vec<f64> = samples.iter().filter(|s| s.t_ms >= half).filter_map(|s| s.clock).collect();
  let clock_mhz = (!late.is_empty()).then(|| late.iter().sum::<f64>() / late.len() as f64);
  let throttled = matches!((clock_mhz, base), (Some(c), Some(b)) if c < b * 0.8);
  let ghz = |m: f64| format!("{:.1} GHz", m / 1000.0);
  let peak = peak_c.map(|t| format!("{t:.0} °C")).unwrap_or_else(|| "an unknown temperature".into());
  let (level, text) = match stop {
    "hot" => ("bad", format!("Stopped at {STOP_C:.0} °C after {seconds} s. The cooling cannot keep up with full load: check the vents are clear and nothing blocks the air underneath.")),
    "no-temp" => ("warn", "Stopped: the temperature reading was lost, so the check could not keep watch. Nothing is known about the cooling yet.".into()),
    "stopped" if seconds < 30 => ("warn", format!("Stopped after {seconds} s, too early to say much. Peak so far {peak}.")),
    _ if throttled => ("warn", format!("Reached {peak} and slowed to {} under load, below its rated {}. The processor holds itself back to stay cool, so long heavy work runs slower.",
      clock_mhz.map(ghz).unwrap_or_default(), base.map(ghz).unwrap_or_default())),
    _ if peak_c.is_some_and(|t| t < 85.0) => ("good", format!("Good. Peak {peak} at full load{}.", clock_mhz.map(|c| format!(", with the clocks holding at {}", ghz(c))).unwrap_or_default())),
    _ => ("good", format!("OK. Peak {peak} at full load, which is normal for a thin laptop{}.", clock_mhz.map(|c| format!("; the clocks held at {}", ghz(c))).unwrap_or_default())),
  };
  Verdict {
    level,
    text: if boost_off { format!("{text} Heat protection turned boost off during the check.") } else { text },
    seconds,
    stop,
    start_c: samples.iter().find_map(|s| s.temp),
    peak_c,
    clock_mhz,
    base_mhz: base,
    peak_w: max_f(samples.iter().filter_map(|s| s.watts)),
    peak_fan: samples.iter().filter_map(|s| s.fan).max(),
    throttled,
    boost_off,
  }
}

/// Sets the stop flag when dropped, so the load threads end even if the check's loop panics. jobs::start catches
/// the panic and reports the job finished, and nothing else would stop the threads.
pub struct StopOnDrop(pub Arc<AtomicBool>);

impl Drop for StopOnDrop {
  fn drop(&mut self) {
    self.0.store(true, Ordering::SeqCst);
  }
}

/// Starts one busy thread per logical processor; they run until `stop` is set.
pub fn load(stop: &Arc<AtomicBool>) -> Vec<std::thread::JoinHandle<()>> {
  let n = std::thread::available_parallelism().map_or(4, |n| n.get());
  (0..n)
    .map(|i| {
      let stop = stop.clone();
      std::thread::spawn(move || {
        unsafe {
          let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
        }
        // Integer and floating-point work together, so both halves of each core get warm.
        let (mut x, mut f) = (i as u64 + 1, [1.0f64 + i as f64; 8]);
        while !stop.load(Ordering::Relaxed) {
          for _ in 0..50_000 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            for v in f.iter_mut() {
              *v = v.mul_add(1.000_000_1, 0.000_000_1);
            }
          }
          std::hint::black_box((x, f));
        }
      })
    })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn run(temps: &[f64], clock: f64) -> Vec<Sample> {
    temps.iter().enumerate().map(|(i, &t)| Sample { t_ms: i as u64 * 1000, temp: Some(t), clock: Some(clock), watts: Some(35.0), fan: Some(4000 + i as u32) }).collect()
  }

  #[test]
  fn stops_for_the_right_reason() {
    assert_eq!(stop_reason(5_000, 90_000, Some(95.0), 0, false), Some("hot"));
    assert_eq!(stop_reason(5_000, 90_000, Some(80.0), 0, true), Some("stopped"));
    assert_eq!(stop_reason(5_000, 90_000, None, 10_000, false), Some("no-temp"));
    assert_eq!(stop_reason(90_000, 90_000, Some(80.0), 0, false), Some("time"));
    assert_eq!(stop_reason(30_000, 90_000, Some(94.9), 9_000, false), None);
    assert_eq!(clamp_secs(5), 60);
    assert_eq!(clamp_secs(500), 120);
  }

  #[test]
  fn verdicts() {
    let cool = verdict(&run(&[50.0, 70.0, 78.0, 80.0], 4000.0), "time", Some(3200.0), false);
    assert_eq!((cool.level, cool.peak_c, cool.throttled, cool.peak_fan), ("good", Some(80.0), false, Some(4003)));
    assert!(cool.text.starts_with("Good."));
    let slow = verdict(&run(&[60.0, 88.0, 90.0, 90.0], 2000.0), "time", Some(3200.0), true);
    assert!(slow.throttled && slow.level == "warn");
    assert!(slow.text.contains("2.0 GHz") && slow.text.ends_with("during the check."));
    let hot = verdict(&run(&[70.0, 95.0], 3500.0), "hot", Some(3200.0), false);
    assert_eq!(hot.level, "bad");
    let early = verdict(&run(&[50.0, 60.0], 3500.0), "stopped", Some(3200.0), false);
    assert!(early.text.contains("too early"));
    assert_eq!(verdict(&[], "stopped", None, false).seconds, 0);
  }

  #[test]
  fn load_stops_even_after_a_panic() {
    let stop = Arc::new(AtomicBool::new(false));
    let threads = load(&stop);
    let s2 = stop.clone();
    let _ = std::panic::catch_unwind(move || {
      let _guard = StopOnDrop(s2);
      panic!("the check's loop failed");
    });
    assert!(stop.load(Ordering::SeqCst), "the guard set the flag while unwinding");
    for t in threads {
      t.join().unwrap();
    }
  }
}
