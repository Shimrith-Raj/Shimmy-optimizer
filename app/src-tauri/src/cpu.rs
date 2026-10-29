//! The CPU section: load per logical processor (sysinfo, sampled by the sampler only), effective clocks (Windows'
//! performance counters, with the LibreHardwareMonitor sidecar as the fallback), temperature and package power (sidecar),
//! fans (ASUS ATK), a plain-language status with throttling detection. The GPU is in gpu.rs.
use crate::sensors::{value, Reading};
use serde::Serialize;
use std::collections::HashMap;
use windows::core::w;
use windows::Win32::System::Performance::{PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhOpenQueryW, PDH_HCOUNTER, PDH_HQUERY};
use windows::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX};

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Core {
  /// The effective clock: how fast the core really runs, averaged over the last sample, not the clock it is set to.
  pub clock_mhz: Option<f64>,
  pub load_pct: f32,
  /// Logical processors on this core: 2 with SMT, 1 without.
  pub threads: usize,
  /// "P-core" or "E-core" on a hybrid processor; None when every core is the same.
  pub kind: Option<&'static str>,
  pub base_mhz: Option<f64>,
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

// ---------- Cores: which logical processors share one ----------

/// One core as `GetLogicalProcessorInformationEx` reports it: its logical processors as (group, bit) masks.
#[derive(Clone, Debug)]
pub struct RawCore {
  pub masks: Vec<(u16, u64)>,
  pub smt: bool,
  /// Windows' EfficiencyClass: higher is faster. Every core is 0 on an ordinary processor.
  pub class: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CoreMap {
  /// (group, index in the group): the names the performance counters use.
  pub lps: Vec<(u16, u16)>,
  /// The same logical processors as positions in the one flat list sysinfo uses (group by group).
  pub flat: Vec<usize>,
  pub smt: bool,
  pub class: u8,
}

/// The core to logical processor map, built once at start.
#[derive(Clone, Debug, Default)]
pub struct Topology {
  pub cores: Vec<CoreMap>,
  pub threads: usize,
}

impl Topology {
  pub fn from_raw(raw: &[RawCore]) -> Topology {
    let mut all: Vec<(u16, u16)> = raw.iter().flat_map(|r| r.masks.iter().flat_map(|&(g, m)| (0..64u16).filter(move |b| m >> b & 1 == 1).map(move |b| (g, b)))).collect();
    all.sort_unstable();
    all.dedup();
    let flat_of: HashMap<(u16, u16), usize> = all.iter().enumerate().map(|(i, lp)| (*lp, i)).collect();
    let mut cores: Vec<CoreMap> = raw
      .iter()
      .map(|r| {
        let mut lps: Vec<(u16, u16)> = r.masks.iter().flat_map(|&(g, m)| (0..64u16).filter(move |b| m >> b & 1 == 1).map(move |b| (g, b))).collect();
        lps.sort_unstable();
        CoreMap { flat: lps.iter().map(|lp| flat_of[lp]).collect(), lps, smt: r.smt, class: r.class }
      })
      .filter(|c| !c.lps.is_empty())
      .collect();
    cores.sort_by_key(|c| c.flat[0]);
    Topology { cores, threads: all.len() }
  }

  /// When Windows will not say which logical processors share a core: each is shown as its own.
  pub fn single_threads(n: usize) -> Topology {
    let raw: Vec<RawCore> = (0..n).map(|i| RawCore { masks: vec![((i / 64) as u16, 1u64 << (i % 64))], smt: false, class: 0 }).collect();
    Topology::from_raw(&raw)
  }

  fn class_range(&self) -> (u8, u8) {
    let classes = self.cores.iter().map(|c| c.class);
    (classes.clone().min().unwrap_or(0), classes.max().unwrap_or(0))
  }

  /// The rated clock of each efficiency class, from the nominal frequency the counters give its logical processors.
  fn class_bases(&self, clocks: &HashMap<(u16, u16), Clock>) -> HashMap<u8, f64> {
    let mut out: HashMap<u8, f64> = HashMap::new();
    for c in &self.cores {
      for base in c.lps.iter().filter_map(|lp| clocks.get(lp)).map(|k| k.base) {
        let e = out.entry(c.class).or_default();
        *e = e.max(base);
      }
    }
    out
  }
}

/// The core map from Windows. None when the call fails (the caller then shows each logical processor on its own).
pub fn topology() -> Option<Topology> {
  const LTP_PC_SMT: u8 = 0x1;
  unsafe {
    let mut len = 0u32;
    // The first call fails by design and reports the size.
    let _ = GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len);
    if len == 0 {
      return None;
    }
    // u64 elements keep the buffer 8-byte aligned, as the records expect.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let base = buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX;
    GetLogicalProcessorInformationEx(RelationProcessorCore, Some(base), &mut len).ok()?;
    let (mut raw, mut at) = (Vec::new(), 0usize);
    while at + std::mem::size_of::<u32>() * 2 <= len as usize {
      let rec = (base as *const u8).add(at) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX;
      let size = std::ptr::addr_of!((*rec).Size).read_unaligned() as usize;
      if size == 0 || at + size > len as usize {
        break;
      }
      if std::ptr::addr_of!((*rec).Relationship).read_unaligned() == RelationProcessorCore {
        let p = std::ptr::addr_of!((*rec).Anonymous.Processor);
        let groups = std::ptr::addr_of!((*p).GroupCount).read_unaligned() as usize;
        let masks = std::ptr::addr_of!((*p).GroupMask) as *const windows::Win32::System::SystemInformation::GROUP_AFFINITY;
        raw.push(RawCore {
          masks: (0..groups).map(|i| masks.add(i).read_unaligned()).map(|g| (g.Group, g.Mask as u64)).collect(),
          smt: std::ptr::addr_of!((*p).Flags).read_unaligned() & LTP_PC_SMT != 0,
          class: std::ptr::addr_of!((*p).EfficiencyClass).read_unaligned(),
        });
      }
      at += size;
    }
    (!raw.is_empty()).then(|| Topology::from_raw(&raw))
  }
}

// ---------- Effective clocks, from Windows' performance counters ----------

/// What one logical processor runs at: `effective` is the clock it really averaged, `base` its nominal clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
  pub effective: f64,
  pub base: f64,
}

/// The latest readings the sampler took, read by everything else (nobody else refreshes them).
#[derive(Default, Clone)]
pub struct Live {
  pub total: f32,
  /// Busy % per logical processor, in sysinfo's flat order.
  pub per_cpu: Vec<f32>,
  pub clocks: HashMap<(u16, u16), Clock>,
}

/// "% Processor Performance" is the share of the nominal clock a processor averaged (above 100 when it boosts).
pub fn effective_mhz(perf_pct: f64, nominal_mhz: f64) -> f64 {
  perf_pct.max(0.0) * nominal_mhz / 100.0
}

/// Counter instance "0,5" is processor 5 of group 0. "_Total" and "0,_Total" are sums, not processors.
fn parse_instance(name: &str) -> Option<(u16, u16)> {
  let (g, i) = name.split_once(',')?;
  Some((g.parse().ok()?, i.parse().ok()?))
}

/// A long-lived PDH query: "% Processor Performance" is a rate, so each read compares with the previous one.
pub struct Perf {
  query: PDH_HQUERY,
  perf: PDH_HCOUNTER,
  freq: PDH_HCOUNTER,
}

// Only the sampler thread uses it.
unsafe impl Send for Perf {}

impl Perf {
  pub fn open() -> Result<Perf, String> {
    unsafe {
      let mut query = PDH_HQUERY::default();
      if PdhOpenQueryW(None, 0, &mut query) != 0 {
        return Err("Windows' processor counters are not available.".into());
      }
      let (mut perf, mut freq) = (PDH_HCOUNTER::default(), PDH_HCOUNTER::default());
      let a = PdhAddEnglishCounterW(query, w!("\\Processor Information(*)\\% Processor Performance"), 0, &mut perf);
      let b = PdhAddEnglishCounterW(query, w!("\\Processor Information(*)\\Processor Frequency"), 0, &mut freq);
      if a != 0 || b != 0 {
        let _ = PdhCloseQuery(query);
        return Err("Windows' processor performance counters are not available.".into());
      }
      // A rate needs two readings; this is the first, the sampler's next tick the second.
      PdhCollectQueryData(query);
      Ok(Perf { query, perf, freq })
    }
  }

  pub fn sample(&mut self) -> HashMap<(u16, u16), Clock> {
    unsafe {
      PdhCollectQueryData(self.query);
    }
    let freq: HashMap<String, f64> = crate::gpu::read(self.freq).into_iter().collect();
    crate::gpu::read(self.perf)
      .into_iter()
      .filter_map(|(name, pct)| {
        let lp = parse_instance(&name)?;
        let base = *freq.get(&name)?;
        (base > 0.0).then(|| (lp, Clock { effective: effective_mhz(pct, base), base }))
      })
      .collect()
  }
}

impl Drop for Perf {
  fn drop(&mut self) {
    unsafe {
      let _ = PdhCloseQuery(self.query);
    }
  }
}

/// Cores from the topology, each with the clock of its fastest logical processor (the siblings share one clock, and an
/// idle sibling reads low), falling back to the sidecar's `Core #N (Effective)`. Both are effective clocks, so the
/// average is what the processor really runs at.
pub fn build(name: String, topo: &Topology, live: &Live, base: Option<f64>, rows: Option<&[Reading]>, fans: Vec<Fan>, sensors_error: Option<String>) -> CpuView {
  let rows = rows.unwrap_or(&[]);
  let (lowest, highest) = topo.class_range();
  let class_base = topo.class_bases(&live.clocks);
  let cores: Vec<Core> = topo
    .cores
    .iter()
    .enumerate()
    .map(|(k, c)| {
      let loads: Vec<f32> = c.flat.iter().filter_map(|i| live.per_cpu.get(*i).copied()).collect();
      let counter = c.lps.iter().filter_map(|lp| live.clocks.get(lp)).map(|k| k.effective).reduce(f64::max);
      Core {
        clock_mhz: counter.or_else(|| value(rows, "Cpu", "Clock", &format!("Core #{} (Effective)", k + 1))),
        load_pct: loads.iter().sum::<f32>() / loads.len().max(1) as f32,
        threads: c.lps.len(),
        kind: (highest > lowest).then(|| if c.class == highest { "P-core" } else { "E-core" }),
        base_mhz: class_base.get(&c.class).copied().or(base),
      }
    })
    .collect();
  let clocks: Vec<f64> = cores.iter().filter_map(|c| c.clock_mhz).collect();
  let avg_clock = (!clocks.is_empty()).then(|| clocks.iter().sum::<f64>() / clocks.len() as f64);
  // The rated clock shown is the fastest class's; the throttling check compares with the average over all cores.
  let rated = topo.cores.iter().position(|c| c.class == highest).and_then(|i| cores[i].base_mhz);
  let bases: Vec<f64> = cores.iter().filter_map(|c| c.base_mhz).collect();
  let avg_base = (!bases.is_empty()).then(|| bases.iter().sum::<f64>() / bases.len() as f64);
  let temp = crate::sensors::cpu_temp(rows);
  CpuView {
    name,
    threads: topo.threads,
    base_mhz: rated,
    load_pct: live.total,
    temp_c: temp,
    package_w: value(rows, "Cpu", "Power", "Package"),
    avg_clock_mhz: avg_clock,
    status: status(temp, live.total, avg_clock, avg_base),
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

  /// `n` cores of two threads each in group 0, like the Ryzen 7 5800HS.
  fn smt_cores(n: u32) -> Topology {
    let raw: Vec<RawCore> = (0..n).map(|k| RawCore { masks: vec![(0, 0b11 << (k * 2))], smt: true, class: 0 }).collect();
    Topology::from_raw(&raw)
  }

  /// Two P-cores with SMT (class 1), then four E-cores without (class 0), as on a hybrid Intel processor.
  fn hybrid() -> Topology {
    let mut raw: Vec<RawCore> = (0..2).map(|k| RawCore { masks: vec![(0, 0b11 << (k * 2))], smt: true, class: 1 }).collect();
    raw.extend((4..8).map(|b| RawCore { masks: vec![(0, 1 << b)], smt: false, class: 0 }));
    Topology::from_raw(&raw)
  }

  #[test]
  fn reads_the_phase0_sensor_dump() {
    let rows = phase0_rows();
    let live = Live { total: 10.0, per_cpu: vec![10.0; 16], ..Default::default() };
    let v = build("Ryzen".into(), &smt_cores(8), &live, Some(3200.0), Some(&rows), vec![], None);
    assert_eq!(v.temp_c, Some(61.25));
    assert_eq!(v.package_w, Some(4.8));
    assert_eq!(v.cores.len(), 8);
    assert_eq!(v.threads, 16);
    // No counters here, so the sidecar's effective clocks are used: 51.5 MHz at idle, not the 1373 MHz the set clock says.
    assert_eq!(v.cores[0].clock_mhz, Some(51.5));
    assert_eq!(v.avg_clock_mhz, Some(50.625));
    assert!(v.sensors_live);
    assert_eq!(v.status.level, "good");
  }

  #[test]
  fn counters_beat_the_sidecar_and_the_fastest_thread_wins() {
    let rows = phase0_rows();
    // Core 1 holds logical processors 0 and 1: one at 50 % of 3200 MHz, its idle sibling at 20 %.
    let clocks = HashMap::from([
      ((0, 0), Clock { effective: effective_mhz(50.0, 3200.0), base: 3200.0 }),
      ((0, 1), Clock { effective: effective_mhz(20.0, 3200.0), base: 3200.0 }),
    ]);
    let live = Live { total: 5.0, per_cpu: vec![40.0, 10.0, 0.0, 0.0], clocks };
    let v = build("Ryzen".into(), &smt_cores(2), &live, None, Some(&rows), vec![], None);
    assert_eq!(v.cores[0].clock_mhz, Some(1600.0));
    assert_eq!(v.cores[0].load_pct, 25.0);
    assert_eq!(v.cores[0].base_mhz, Some(3200.0));
    // Core 2 has no counter reading, so it falls back to the sidecar's "Core #2 (Effective)".
    assert_eq!(v.cores[1].clock_mhz, Some(80.5));
    assert_eq!(effective_mhz(130.0, 3200.0), 4160.0, "boost reads above the nominal clock");
    assert_eq!(effective_mhz(-1.0, 3200.0), 0.0);
  }

  #[test]
  fn works_without_sensors() {
    let live = Live { total: 40.0, per_cpu: vec![50.0, 30.0], ..Default::default() };
    let v = build("Ryzen".into(), &smt_cores(1), &live, None, None, vec![], Some("not admin".into()));
    assert_eq!(v.temp_c, None);
    assert_eq!(v.cores.len(), 1);
    assert_eq!(v.cores[0].load_pct, 40.0);
    assert_eq!(v.cores[0].clock_mhz, None);
    assert_eq!(v.avg_clock_mhz, None);
    assert_eq!(v.status, Status { level: "good", text: "Running normally at 40% load.".into() });
  }

  #[test]
  fn hybrid_cores_are_paired_by_windows_not_by_position() {
    let t = hybrid();
    assert_eq!(t.threads, 8);
    assert_eq!(t.cores.len(), 6);
    assert_eq!(t.cores[0].flat, vec![0, 1]);
    assert_eq!(t.cores[2].flat, vec![4], "an E-core is a single thread");
    let live = Live {
      total: 20.0,
      per_cpu: vec![10.0, 30.0, 0.0, 0.0, 60.0, 0.0, 0.0, 0.0],
      clocks: HashMap::from([
        ((0, 0), Clock { effective: 4000.0, base: 2400.0 }),
        ((0, 4), Clock { effective: 1900.0, base: 1800.0 }),
      ]),
    };
    let v = build("Core".into(), &t, &live, Some(2000.0), None, vec![], None);
    assert_eq!(v.cores.iter().map(|c| c.kind).collect::<Vec<_>>(), vec![Some("P-core"), Some("P-core"), Some("E-core"), Some("E-core"), Some("E-core"), Some("E-core")]);
    assert_eq!(v.cores[0].load_pct, 20.0);
    assert_eq!(v.cores[2].load_pct, 60.0);
    assert_eq!(v.cores[2].threads, 1);
    assert_eq!(v.cores[0].base_mhz, Some(2400.0));
    assert_eq!(v.cores[2].base_mhz, Some(1800.0), "base clock per efficiency class");
    assert_eq!(v.cores[3].base_mhz, Some(1800.0));
    assert_eq!(v.base_mhz, Some(2400.0), "the rated clock shown is the P-cores'");
    // A uniform processor shows no kind at all.
    assert!(build("Ryzen".into(), &smt_cores(2), &Live::default(), None, None, vec![], None).cores.iter().all(|c| c.kind.is_none()));
  }

  #[test]
  fn flat_positions_run_group_by_group() {
    // Group 0 holds processors 0-3 (two cores), group 1 holds 0-1 (one core): flat positions 4 and 5.
    let raw = vec![
      RawCore { masks: vec![(1, 0b11)], smt: true, class: 0 },
      RawCore { masks: vec![(0, 0b1100)], smt: true, class: 0 },
      RawCore { masks: vec![(0, 0b0011)], smt: true, class: 0 },
    ];
    let t = Topology::from_raw(&raw);
    assert_eq!(t.cores.iter().map(|c| c.flat.clone()).collect::<Vec<_>>(), vec![vec![0, 1], vec![2, 3], vec![4, 5]]);
    assert_eq!(t.cores[2].lps, vec![(1, 0), (1, 1)]);
    assert_eq!(Topology::single_threads(3).cores.len(), 3);
  }

  #[test]
  fn counter_instances() {
    assert_eq!(parse_instance("0,12"), Some((0, 12)));
    assert_eq!(parse_instance("1,0"), Some((1, 0)));
    assert_eq!(parse_instance("_Total"), None);
    assert_eq!(parse_instance("0,_Total"), None);
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
