//! The Graphics page. The NVIDIA GPU in detail through NVML, read only while a page asks, so the RTX can sleep the
//! rest of the time. Per-engine and per-app GPU use for every adapter (the Radeon too) come from Windows' own
//! "GPU Engine" counters, the ones Task Manager uses; they are kept by the graphics kernel and do not wake the GPU.
use serde::Serialize;
use std::collections::HashMap;
use windows::core::w;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::System::Performance::*;

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Nvidia {
  pub name: String,
  pub driver: Option<String>,
  pub temp_c: Option<u32>,
  /// Where the GPU starts slowing itself down to cool off.
  pub slowdown_c: Option<u32>,
  pub load_pct: Option<u32>,
  /// How busy the video memory bus is.
  pub mem_load_pct: Option<u32>,
  pub encoder_pct: Option<u32>,
  pub decoder_pct: Option<u32>,
  pub clock_mhz: Option<u32>,
  pub max_clock_mhz: Option<u32>,
  pub mem_clock_mhz: Option<u32>,
  pub max_mem_clock_mhz: Option<u32>,
  pub power_w: Option<f64>,
  pub power_limit_w: Option<f64>,
  pub vram_used_mb: Option<u64>,
  pub vram_total_mb: Option<u64>,
  /// 0 is full speed, 8 and above is resting.
  pub pstate: Option<u32>,
  pub pcie: Option<String>,
  /// What is holding the clock back right now, in plain words.
  pub limits: Vec<String>,
  pub idle: bool,
  pub status: Status,
}

#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Status {
  pub level: &'static str,
  pub text: String,
}

pub fn nvidia(nvml: &nvml_wrapper::Nvml) -> Result<Nvidia, String> {
  use nvml_wrapper::bitmasks::device::ThrottleReasons as T;
  use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor, TemperatureThreshold};
  let d = nvml.device_by_index(0).map_err(|e| e.to_string())?;
  let memory = d.memory_info().ok();
  let util = d.utilization_rates().ok();
  let reasons = d.current_throttle_reasons().unwrap_or(T::NONE);
  let mut limits = Vec::new();
  for (flag, words) in [
    (T::SW_POWER_CAP, "power limit"),
    (T::HW_POWER_BRAKE_SLOWDOWN, "power brake from the laptop"),
    (T::SW_THERMAL_SLOWDOWN | T::HW_THERMAL_SLOWDOWN, "heat"),
    (T::HW_SLOWDOWN, "hardware slowdown"),
  ] {
    if reasons.intersects(flag) && !limits.contains(&words.to_string()) {
      limits.push(words.to_string());
    }
  }
  let pstate = d.performance_state().ok().and_then(|p| {
    let n = p as u32;
    (n <= 15).then_some(n)
  });
  let pcie = match (d.current_pcie_link_gen(), d.current_pcie_link_width()) {
    (Ok(g), Ok(w)) => Some(format!("PCIe {g}.0 x{w}")),
    _ => None,
  };
  let mut g = Nvidia {
    name: d.name().unwrap_or_else(|_| "NVIDIA GPU".into()),
    driver: nvml.sys_driver_version().ok(),
    temp_c: d.temperature(TemperatureSensor::Gpu).ok(),
    slowdown_c: d.temperature_threshold(TemperatureThreshold::Slowdown).ok(),
    load_pct: util.as_ref().map(|u| u.gpu),
    mem_load_pct: util.as_ref().map(|u| u.memory),
    encoder_pct: d.encoder_utilization().ok().map(|u| u.utilization),
    decoder_pct: d.decoder_utilization().ok().map(|u| u.utilization),
    clock_mhz: d.clock_info(Clock::Graphics).ok(),
    max_clock_mhz: d.max_clock_info(Clock::Graphics).ok(),
    mem_clock_mhz: d.clock_info(Clock::Memory).ok(),
    max_mem_clock_mhz: d.max_clock_info(Clock::Memory).ok(),
    power_w: d.power_usage().ok().map(|mw| mw as f64 / 1000.0),
    power_limit_w: d.enforced_power_limit().ok().map(|mw| mw as f64 / 1000.0),
    vram_used_mb: memory.as_ref().map(|m| m.used / 1_048_576),
    vram_total_mb: memory.as_ref().map(|m| m.total / 1_048_576),
    pstate,
    pcie,
    limits,
    idle: reasons.contains(T::GPU_IDLE),
    status: Status::default(),
  };
  g.status = status(&g);
  Ok(g)
}

/// A plain-language verdict, like the processor's.
pub fn status(g: &Nvidia) -> Status {
  let load = g.load_pct.unwrap_or(0);
  let hot_at = g.slowdown_c.unwrap_or(90).saturating_sub(5);
  let s = |level, text: String| Status { level, text };
  if let Some(t) = g.temp_c {
    if g.limits.iter().any(|l| l == "heat") || t >= hot_at {
      return s("bad", format!("Running hot at {t} °C and slowing itself down to cool off. Check the vents are clear."));
    }
    if t >= 80 {
      return s("warn", format!("Warm at {t} °C. Normal for long gaming or rendering, but worth watching."));
    }
  }
  if load < 3 && (g.idle || g.pstate.is_some_and(|p| p >= 8)) {
    return s("good", "Resting. The graphics card is idle and saving power.".into());
  }
  if load >= 50 && g.limits.iter().any(|l| l.starts_with("power")) {
    return s("good", format!("Working hard at {load}% and held at its power limit, which is normal for a laptop under full load."));
  }
  s("good", format!("Working at {load}%, temperature fine."))
}

// ---------- Windows GPU counters, all adapters ----------

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
  pub luid: String,
  pub name: String,
  pub short: String,
  /// Integrated graphics share the main memory.
  pub integrated: bool,
  pub load_pct: f64,
  pub engines: Vec<Engine>,
  pub dedicated_mb: u64,
  pub shared_mb: u64,
  pub dedicated_total_mb: u64,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Engine {
  pub kind: String,
  pub pct: f64,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppUse {
  pub pid: u32,
  pub app: String,
  pub pct: f64,
  pub adapter: String,
  pub engine: String,
}

struct AdapterInfo {
  name: String,
  dedicated_total: u64,
}

/// A long-lived PDH query: utilisation is a rate, so each read compares with the previous one.
pub struct Counters {
  query: PDH_HQUERY,
  engine: PDH_HCOUNTER,
  dedicated: PDH_HCOUNTER,
  shared: PDH_HCOUNTER,
  adapters: HashMap<String, AdapterInfo>,
}

// The handles are only used under Shared's mutex.
unsafe impl Send for Counters {}

/// "luid_0x00000000_0x0000D1B5" from any instance name holding it.
fn luid_of(instance: &str) -> Option<String> {
  let i = instance.find("luid_")?;
  let rest = &instance[i + 5..];
  let parts: Vec<&str> = rest.split('_').take(2).collect();
  if parts.len() < 2 {
    return None;
  }
  let hi = u32::from_str_radix(parts[0].trim_start_matches("0x"), 16).ok()?;
  let lo = u32::from_str_radix(parts[1].trim_start_matches("0x"), 16).ok()?;
  Some(format!("{hi:08X}{lo:08X}"))
}

/// Instance "pid_1234_luid_0x0_0xD1B5_phys_0_eng_3_engtype_VideoDecode" → (pid, luid, engine index, engine type).
fn parse_engine(instance: &str) -> Option<(u32, String, u32, String)> {
  let pid = instance.strip_prefix("pid_")?.split('_').next()?.parse().ok()?;
  let luid = luid_of(instance)?;
  let eng = instance.split("_eng_").nth(1)?.split('_').next()?.parse().ok()?;
  let kind = instance.split("engtype_").nth(1)?.to_string();
  Some((pid, luid, eng, kind))
}

/// Drivers name engines freely ("Video Decode 1", "VideoDecode", "OFA 0"); the main kinds are merged, the rest are
/// "Other" (still counted in the adapter's load, not shown as an engine).
fn friendly_engine(kind: &str) -> &'static str {
  let k: String = kind.chars().filter(|c| c.is_ascii_alphabetic() || *c == '3').collect::<String>().to_lowercase();
  match k.as_str() {
    "3d" => "3D",
    "copy" => "Copy",
    "videodecode" => "Video decode",
    "videoencode" => "Video encode",
    "videocodec" => "Video codec",
    "videoprocessing" => "Video processing",
    "compute" | "highprioritycompute" => "Compute",
    _ => "Other",
  }
}

fn short_name(name: &str) -> String {
  name.replace("NVIDIA GeForce ", "").replace("Laptop GPU", "").replace("(TM)", "").replace("(R)", "").trim().to_string()
}

fn dxgi_adapters() -> HashMap<String, AdapterInfo> {
  let mut out = HashMap::new();
  let Ok(f) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return out };
  for i in 0.. {
    let Ok(a) = (unsafe { f.EnumAdapters1(i) }) else { break };
    let Ok(d) = (unsafe { a.GetDesc1() }) else { continue };
    if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
      continue;
    }
    let name = String::from_utf16_lossy(&d.Description[..d.Description.iter().position(|c| *c == 0).unwrap_or(0)]).trim().to_string();
    let luid = format!("{:08X}{:08X}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
    out.insert(luid, AdapterInfo { name, dedicated_total: d.DedicatedVideoMemory as u64 });
  }
  out
}

pub(crate) fn read(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
  unsafe {
    let (mut size, mut count) = (0u32, 0u32);
    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None) != PDH_MORE_DATA {
      return Vec::new();
    }
    let n = size as usize / std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() + 1;
    let mut buf: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> = vec![Default::default(); n];
    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(buf.as_mut_ptr())) != 0 {
      return Vec::new();
    }
    buf.truncate(count as usize);
    buf.iter()
      .filter(|i| i.FmtValue.CStatus <= 1)
      .map(|i| (i.szName.to_string().unwrap_or_default(), i.FmtValue.Anonymous.doubleValue))
      .collect()
  }
}

impl Counters {
  pub fn open() -> Result<Counters, String> {
    unsafe {
      let mut query = PDH_HQUERY::default();
      if PdhOpenQueryW(None, 0, &mut query) != 0 {
        return Err("Windows' GPU counters are not available.".into());
      }
      let (mut engine, mut dedicated, mut shared) = (PDH_HCOUNTER::default(), PDH_HCOUNTER::default(), PDH_HCOUNTER::default());
      let a = PdhAddEnglishCounterW(query, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut engine);
      let _ = PdhAddEnglishCounterW(query, w!("\\GPU Adapter Memory(*)\\Dedicated Usage"), 0, &mut dedicated);
      let _ = PdhAddEnglishCounterW(query, w!("\\GPU Adapter Memory(*)\\Shared Usage"), 0, &mut shared);
      if a != 0 {
        let _ = PdhCloseQuery(query);
        return Err("Windows' GPU engine counters are not available.".into());
      }
      // A rate needs two readings; take the first now so the first page load already shows real numbers.
      PdhCollectQueryData(query);
      std::thread::sleep(std::time::Duration::from_millis(300));
      Ok(Counters { query, engine, dedicated, shared, adapters: dxgi_adapters() })
    }
  }

  /// Per-adapter engine use and the top apps. `names` maps pid → app name.
  pub fn sample(&mut self, names: &HashMap<u32, String>) -> (Vec<Adapter>, Vec<AppUse>) {
    unsafe {
      PdhCollectQueryData(self.query);
    }
    summarise(&read(self.engine), &read(self.dedicated), &read(self.shared), &self.adapters, names)
  }
}

impl Drop for Counters {
  fn drop(&mut self) {
    unsafe {
      let _ = PdhCloseQuery(self.query);
    }
  }
}

/// Task Manager's arithmetic: an engine's use is the sum over processes; an engine type shows its busiest engine; an
/// adapter shows its busiest engine type; an app shows its busiest engine.
fn summarise(engines: &[(String, f64)], dedicated: &[(String, f64)], shared: &[(String, f64)], info: &HashMap<String, AdapterInfo>, names: &HashMap<u32, String>) -> (Vec<Adapter>, Vec<AppUse>) {
  let mut per_engine: HashMap<(String, u32, String), f64> = HashMap::new();
  let mut per_app: HashMap<(u32, String), (f64, String)> = HashMap::new();
  for (inst, v) in engines {
    let Some((pid, luid, eng, kind)) = parse_engine(inst) else { continue };
    let v = v.clamp(0.0, 100.0);
    *per_engine.entry((luid.clone(), eng, kind.clone())).or_default() += v;
    let e = per_app.entry((pid, luid)).or_insert((0.0, String::new()));
    if v > e.0 {
      *e = (v, kind);
    }
  }
  let mem = |list: &[(String, f64)]| -> HashMap<String, u64> {
    let mut m = HashMap::new();
    for (inst, v) in list {
      if let Some(l) = luid_of(inst) {
        *m.entry(l).or_default() += (*v / 1_048_576.0) as u64;
      }
    }
    m
  };
  let (ded, sha) = (mem(dedicated), mem(shared));

  let mut luids: Vec<String> = per_engine.keys().map(|k| k.0.clone()).chain(info.keys().cloned()).collect();
  luids.sort();
  luids.dedup();
  let mut adapters: Vec<Adapter> = luids
    .into_iter()
    .filter(|l| info.contains_key(l))
    .map(|l| {
      let mut by_kind: HashMap<&str, f64> = HashMap::new();
      for ((al, _, kind), v) in &per_engine {
        if *al == l {
          let k = by_kind.entry(friendly_engine(kind)).or_default();
          *k = k.max(v.min(100.0));
        }
      }
      let load = by_kind.values().copied().fold(0.0, f64::max);
      let mut engines: Vec<Engine> =
        by_kind.into_iter().filter(|(k, _)| *k != "Other").map(|(kind, pct)| Engine { kind: kind.into(), pct: (pct * 10.0).round() / 10.0 }).collect();
      let order = ["3D", "Copy", "Video decode", "Video encode", "Video codec", "Video processing", "Compute"];
      engines.sort_by_key(|e| order.iter().position(|o| *o == e.kind).unwrap_or(99));
      let a = &info[&l];
      Adapter {
        load_pct: (load * 10.0).round() / 10.0,
        name: a.name.replace("(TM)", "").replace("(R)", ""),
        short: short_name(&a.name),
        integrated: a.dedicated_total < 1024 * 1_048_576,
        dedicated_total_mb: a.dedicated_total / 1_048_576,
        dedicated_mb: ded.get(&l).copied().unwrap_or(0),
        shared_mb: sha.get(&l).copied().unwrap_or(0),
        engines,
        luid: l,
      }
    })
    .collect();
  // Discrete GPU first.
  adapters.sort_by_key(|a| a.integrated);

  let short = |l: &str| info.get(l).map(|a| short_name(&a.name)).unwrap_or_default();
  let mut apps: Vec<AppUse> = per_app
    .into_iter()
    .filter(|(_, (v, _))| *v >= 0.5)
    .map(|((pid, luid), (pct, kind))| AppUse {
      pid,
      app: names.get(&pid).cloned().unwrap_or_else(|| format!("Process {pid}")),
      pct: (pct * 10.0).round() / 10.0,
      adapter: short(&luid),
      engine: friendly_engine(&kind).into(),
    })
    .collect();
  apps.sort_by(|a, b| b.pct.total_cmp(&a.pct));
  apps.truncate(10);
  (adapters, apps)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_engine_instances() {
    let i = "pid_1234_luid_0x00000000_0x0000D1B5_phys_0_eng_3_engtype_VideoDecode";
    assert_eq!(parse_engine(i), Some((1234, "000000000000D1B5".into(), 3, "VideoDecode".into())));
    assert_eq!(luid_of("luid_0x00000000_0x0000D1B5_phys_0"), Some("000000000000D1B5".into()));
    assert_eq!(parse_engine("garbage"), None);
    assert_eq!(friendly_engine("Video Decode 1"), "Video decode");
    assert_eq!(friendly_engine("VideoDecode"), "Video decode");
    assert_eq!(friendly_engine("OFA 0"), "Other");
  }

  #[test]
  fn sums_like_task_manager() {
    let dgpu = "000000000000AAAA".to_string();
    let igpu = "000000000000BBBB".to_string();
    let info = HashMap::from([
      (dgpu.clone(), AdapterInfo { name: "NVIDIA GeForce RTX 3050 Laptop GPU".into(), dedicated_total: 4 << 30 }),
      (igpu.clone(), AdapterInfo { name: "AMD Radeon(TM) Graphics".into(), dedicated_total: 512 << 20 }),
    ]);
    let e = |pid: u32, l: &str, eng: u32, kind: &str, v: f64| (format!("pid_{pid}_luid_0x00000000_0x0000{l}_phys_0_eng_{eng}_engtype_{kind}"), v);
    let engines = vec![e(10, "AAAA", 0, "3D", 30.0), e(11, "AAAA", 0, "3D", 25.0), e(11, "AAAA", 5, "VideoDecode", 12.0), e(12, "BBBB", 0, "3D", 8.0), e(13, "BBBB", 0, "3D", 0.1)];
    let names = HashMap::from([(10, "Game".to_string()), (11, "Brave".to_string())]);
    let (adapters, apps) = summarise(&engines, &[("luid_0x00000000_0x0000AAAA_phys_0".into(), 1_048_576.0 * 900.0)], &[], &info, &names);
    assert_eq!(adapters[0].name, "NVIDIA GeForce RTX 3050 Laptop GPU");
    assert_eq!(adapters[1].short, "AMD Radeon Graphics");
    assert_eq!(adapters[0].load_pct, 55.0);
    assert_eq!(adapters[0].dedicated_mb, 900);
    assert_eq!(adapters[0].engines[0].kind, "3D");
    assert_eq!(adapters[0].engines[1].kind, "Video decode");
    assert!(adapters[1].integrated);
    assert_eq!(apps.iter().map(|a| (a.app.as_str(), a.pct)).collect::<Vec<_>>(), vec![("Game", 30.0), ("Brave", 25.0), ("Process 12", 8.0)]);
    assert_eq!(apps[0].adapter, "RTX 3050");
  }

  #[test]
  fn status_words() {
    let mut g = Nvidia { temp_c: Some(50), load_pct: Some(0), pstate: Some(8), ..Default::default() };
    assert!(status(&g).text.starts_with("Resting"));
    g = Nvidia { temp_c: Some(74), load_pct: Some(97), limits: vec!["power limit".into()], ..Default::default() };
    assert!(status(&g).text.contains("power limit"));
    g = Nvidia { temp_c: Some(88), slowdown_c: Some(92), load_pct: Some(97), ..Default::default() };
    assert_eq!(status(&g).level, "bad");
  }
}
