//! The Graphics page, for every graphics chip (AMD, Intel, NVIDIA).
//! - Per-engine and per-app use for every adapter come from Windows' own "GPU Engine" counters, the ones Task Manager
//!   uses; they are kept by the graphics kernel and do not wake a GPU.
//! - Temperature, clocks, fan and power for any vendor come from the graphics kernel (D3DKMT adapter and engine
//!   performance data, the source of Task Manager's GPU temperature). A dedicated card on a laptop with a built-in chip is
//!   asked only while Windows' counters show it in use, so Shimmy never wakes it; built-in chips are asked whenever the
//!   page is open.
//! - NVIDIA cards also get NVML, read only while a page asks: richer (watts, P-state, what holds the clock back), and its
//!   values win where both answer.
use serde::Serialize;
use std::collections::HashMap;
use windows::core::w;
use windows::Wdk::Graphics::Direct3D::{
  D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, D3DKMT_ADAPTERTYPE, D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER,
  D3DKMT_NODE_PERFDATA, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO, KMTQAITYPE_ADAPTERPERFDATA, KMTQAITYPE_ADAPTERTYPE,
  KMTQAITYPE_NODEPERFDATA, KMTQUERYADAPTERINFOTYPE,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::System::Performance::*;

const NVIDIA: u32 = 0x10DE;
const AMD: u32 = 0x1002;
const INTEL: u32 = 0x8086;

#[derive(Serialize, Debug, Default, Clone)]
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

#[derive(Serialize, Debug, Default, PartialEq, Clone)]
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
  /// "AMD", "Intel", "NVIDIA" or "" when the vendor is not one of those.
  pub vendor: String,
  /// Integrated graphics share the main memory.
  pub integrated: bool,
  /// A dedicated card that Windows' counters show idle, on a PC that also has a built-in chip, so it may be powered
  /// down. Nothing is asked of it through the graphics kernel while this is true.
  pub asleep: bool,
  pub load_pct: f64,
  pub engines: Vec<Engine>,
  pub dedicated_mb: u64,
  pub shared_mb: u64,
  pub dedicated_total_mb: u64,
  /// Temperature, clocks, fan and power: NVML and the graphics kernel merged. None when nothing could be asked.
  pub readout: Option<Readout>,
  /// The NVIDIA-only detail (P-state, what holds the clock back, PCIe link, encoder and decoder) when NVML answers.
  pub nvidia: Option<Nvidia>,
  pub status: Status,
  #[serde(skip)]
  vendor_id: u32,
  /// The engine (node) that Windows names 3D, whose clock is the adapter's clock.
  #[serde(skip)]
  node_3d: Option<u32>,
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
  vendor_id: u32,
  dedicated_total: u64,
  /// The graphics kernel's adapter type bits, once the adapter has been asked (see `is_integrated`).
  kmt_type: Option<u32>,
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

/// The key `luid_of` and `dxgi_adapters` make ("000000000000D1B5") back into LUID parts: (HighPart, LowPart).
fn luid_parts(key: &str) -> Option<(i32, u32)> {
  if key.len() != 16 || !key.is_ascii() {
    return None;
  }
  let hi = u32::from_str_radix(&key[..8], 16).ok()?;
  let lo = u32::from_str_radix(&key[8..], 16).ok()?;
  Some((hi as i32, lo))
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

fn vendor_name(id: u32) -> &'static str {
  match id {
    NVIDIA => "NVIDIA",
    AMD => "AMD",
    INTEL => "Intel",
    _ => "",
  }
}

// The graphics kernel's adapter type bits (D3DKMT_ADAPTERTYPE): set on hybrid laptops, one chip each.
const HYBRID_DISCRETE: u32 = 1 << 4;
const HYBRID_INTEGRATED: u32 = 1 << 5;

/// Built-in chip or dedicated card. The kernel's hybrid bits decide when known; before an adapter has been asked, NVIDIA
/// is always dedicated and anything else is built in when it has under 1 GB of its own memory. An AMD or Intel chip with a
/// large memory carve-out therefore starts as dedicated, which is the safe side: it is only asked while in use, and the
/// first answer corrects it.
fn is_integrated(vendor_id: u32, dedicated_bytes: u64, kmt_type: Option<u32>) -> bool {
  if let Some(t) = kmt_type {
    if t & HYBRID_INTEGRATED != 0 {
      return true;
    }
    if t & HYBRID_DISCRETE != 0 {
      return false;
    }
  }
  vendor_id != NVIDIA && dedicated_bytes < 1 << 30
}

/// Below this, Windows' counters count a dedicated card as idle (the same line the Battery page uses).
pub const IN_USE_PCT: f64 = 1.0;

/// Whether a dedicated card may be asleep, so nothing may be asked of it: idle, on a PC that has a built-in chip to run
/// the screen. Without a built-in chip the dedicated card runs the screen and is always awake, so asking costs nothing.
fn may_sleep(integrated: bool, load_pct: f64, has_builtin: bool) -> bool {
  !integrated && has_builtin && load_pct < IN_USE_PCT
}

/// The built-in chip, for the sleep decision. When unsure, the safe answer is "built in", so the NVIDIA card may sleep and
/// is not asked: on a PC with an NVIDIA card, any other chip is taken as the built-in one unless the kernel's hybrid bits
/// say it is the dedicated one. (A hybrid laptop's AMD or Intel chip can report 1 GB or more of its own memory.)
fn is_builtin(vendor_id: u32, dedicated_bytes: u64, kmt_type: Option<u32>, has_nvidia: bool) -> bool {
  if is_integrated(vendor_id, dedicated_bytes, kmt_type) {
    return true;
  }
  has_nvidia && vendor_id != NVIDIA && kmt_type.is_none_or(|t| t & HYBRID_DISCRETE == 0)
}

/// Sets each adapter's `asleep` from its load and whether the PC has a built-in chip.
fn set_sleep(adapters: &mut [Adapter]) {
  let has_builtin = adapters.iter().any(|a| a.integrated);
  for a in adapters {
    a.asleep = may_sleep(a.integrated, a.load_pct, has_builtin);
  }
}

/// Whether NVML may be asked: only when an NVIDIA chip is awake (in use by Windows' counters). Asking NVML wakes a
/// sleeping card, and with no adapter list at all nothing is known, so it is not asked blindly.
pub fn nvml_wanted(adapters: &[Adapter]) -> bool {
  adapters.iter().any(|a| a.vendor_id == NVIDIA && !a.asleep)
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
    out.insert(luid, AdapterInfo { name, vendor_id: d.VendorId, dedicated_total: d.DedicatedVideoMemory as u64, kmt_type: None });
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

  /// Per-adapter engine use and the top apps. `names` maps pid → app name. Reads only Windows' counters.
  pub fn sample(&mut self, names: &HashMap<u32, String>) -> (Vec<Adapter>, Vec<AppUse>) {
    unsafe {
      PdhCollectQueryData(self.query);
    }
    summarise(&read(self.engine), &read(self.dedicated), &read(self.shared), &self.adapters, names)
  }

  /// Remembers the kernel's hybrid bits (LUID, type) that `kernel_readouts` learned, so built in or dedicated is right
  /// from now on.
  pub fn learn(&mut self, types: &[(String, u32)]) {
    for (luid, t) in types {
      if let Some(info) = self.adapters.get_mut(luid) {
        info.kmt_type = Some(*t);
      }
    }
  }
}

/// Fills each adapter's readout from the graphics kernel, for every adapter that may be asked (never a sleeping card, see
/// `may_sleep`). Takes no lock, so callers do it after letting go of `Counters`. Returns the adapter types it learned, for
/// `Counters::learn`. The sleep verdicts are worked out again with them.
pub fn kernel_readouts(adapters: &mut [Adapter]) -> Vec<(String, u32)> {
  let mut learned = Vec::new();
  let has_nvidia = adapters.iter().any(|a| a.vendor_id == NVIDIA);
  for a in adapters.iter_mut() {
    let kmt = if a.asleep { None } else { kmt::query(&a.luid, a.node_3d) };
    if let Some(t) = kmt.as_ref().and_then(|k| k.adapter_type) {
      learned.push((a.luid.clone(), t));
      a.integrated = is_builtin(a.vendor_id, a.dedicated_total_mb << 20, Some(t), has_nvidia);
    }
    a.readout = merge(None, kmt.map(|k| k.values).as_ref());
  }
  set_sleep(adapters);
  learned
}

/// NVML and the verdicts only, no graphics kernel. Also the whole job when Windows' counters are not available.
pub fn readouts_without_counters(adapters: &mut Vec<Adapter>, nv: Option<Nvidia>) {
  if let Some(nv) = nv {
    attach_nvidia(adapters, nv);
  }
  for a in adapters.iter_mut() {
    a.status = adapter_status(a);
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
  let has_nvidia = info.values().any(|a| a.vendor_id == NVIDIA);
  let mut adapters: Vec<Adapter> = luids
    .into_iter()
    .filter(|l| info.contains_key(l))
    .map(|l| {
      let mut by_kind: HashMap<&str, f64> = HashMap::new();
      let mut node_3d: Option<u32> = None;
      for ((al, eng, kind), v) in &per_engine {
        if *al == l {
          let f = friendly_engine(kind);
          let k = by_kind.entry(f).or_default();
          *k = k.max(v.min(100.0));
          if f == "3D" {
            node_3d = Some(node_3d.map_or(*eng, |n| n.min(*eng)));
          }
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
        vendor: vendor_name(a.vendor_id).into(),
        integrated: is_builtin(a.vendor_id, a.dedicated_total, a.kmt_type, has_nvidia),
        asleep: false,
        dedicated_total_mb: a.dedicated_total / 1_048_576,
        dedicated_mb: ded.get(&l).copied().unwrap_or(0),
        shared_mb: sha.get(&l).copied().unwrap_or(0),
        engines,
        readout: None,
        nvidia: None,
        status: Status::default(),
        vendor_id: a.vendor_id,
        node_3d,
        luid: l,
      }
    })
    .collect();
  set_sleep(&mut adapters);
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

// ---------- Readouts: the graphics kernel (any vendor) merged with NVML ----------

/// One adapter's values from the graphics kernel, already cleaned (see `clean`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Kmt {
  pub temp_c: Option<f64>,
  pub fan_rpm: Option<u32>,
  /// Share of the card's power budget, in %. The kernel reports no watts.
  pub power_pct: Option<f64>,
  pub clock_mhz: Option<u32>,
  pub max_clock_mhz: Option<u32>,
  pub mem_clock_mhz: Option<u32>,
  pub max_mem_clock_mhz: Option<u32>,
}

/// The raw numbers as the kernel gives them, in its units, so the cleaning rules are testable without Windows.
#[derive(Debug, Default, Clone, Copy)]
struct RawPerf {
  /// Tenths of a °C.
  temperature: u32,
  fan_rpm: u32,
  /// Tenths of a % of the power budget.
  power: u32,
  mem_hz: u64,
  max_mem_hz: u64,
}

#[derive(Debug, Default, Clone, Copy)]
struct RawNode {
  hz: u64,
  max_hz: u64,
}

/// Turns the kernel's numbers into readings. A driver that does not support a field leaves it at 0, and a real GPU is
/// never at 0 °C, 0 RPM with its fan reported, 0 % power or 0 MHz while it answers, so 0 means "not reported" and becomes
/// None (shown as unavailable, never as 0 °C). Values outside what any GPU does are dropped the same way.
fn clean(p: Option<RawPerf>, n: Option<RawNode>) -> Kmt {
  let mhz = |hz: u64| -> Option<u32> {
    let m = hz / 1_000_000;
    (1..=20_000).contains(&m).then_some(m as u32)
  };
  let p = p.unwrap_or_default();
  let n = n.unwrap_or_default();
  Kmt {
    temp_c: (1..=1500).contains(&p.temperature).then(|| p.temperature as f64 / 10.0),
    fan_rpm: (1..=20_000).contains(&p.fan_rpm).then_some(p.fan_rpm),
    power_pct: (1..=2000).contains(&p.power).then(|| p.power as f64 / 10.0),
    clock_mhz: mhz(n.hz),
    max_clock_mhz: mhz(n.max_hz),
    mem_clock_mhz: mhz(p.mem_hz),
    max_mem_clock_mhz: mhz(p.max_mem_hz),
  }
}

/// The 3D node's clock, asked only when Windows named a 3D engine: node 0 is not assumed to be it.
fn node_clock(node: Option<u32>, ask: impl FnOnce(u32) -> Option<RawNode>) -> Option<RawNode> {
  node.and_then(ask)
}

/// What the Graphics page shows for one adapter, whichever source answered.
#[derive(Serialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Readout {
  pub temp_c: Option<f64>,
  pub fan_rpm: Option<u32>,
  /// Watts, from NVML only.
  pub power_w: Option<f64>,
  pub power_limit_w: Option<f64>,
  /// Share of the power budget, from the graphics kernel; only kept when there are no watts.
  pub power_pct: Option<f64>,
  pub clock_mhz: Option<u32>,
  pub max_clock_mhz: Option<u32>,
  pub mem_clock_mhz: Option<u32>,
  pub max_mem_clock_mhz: Option<u32>,
}

/// NVML wins where it has a value, the graphics kernel fills the gaps. None when neither answered with anything.
fn merge(nv: Option<&Nvidia>, k: Option<&Kmt>) -> Option<Readout> {
  let k = k.cloned().unwrap_or_default();
  let power_w = nv.and_then(|g| g.power_w);
  let r = Readout {
    temp_c: nv.and_then(|g| g.temp_c).map(f64::from).or(k.temp_c),
    fan_rpm: k.fan_rpm,
    power_w,
    power_limit_w: nv.and_then(|g| g.power_limit_w),
    power_pct: if power_w.is_some() { None } else { k.power_pct },
    clock_mhz: nv.and_then(|g| g.clock_mhz).or(k.clock_mhz),
    max_clock_mhz: nv.and_then(|g| g.max_clock_mhz).or(k.max_clock_mhz),
    mem_clock_mhz: nv.and_then(|g| g.mem_clock_mhz).or(k.mem_clock_mhz),
    max_mem_clock_mhz: nv.and_then(|g| g.max_mem_clock_mhz).or(k.max_mem_clock_mhz),
  };
  (r != Readout::default()).then_some(r)
}

/// Puts NVML's answer on the NVIDIA adapter it describes (same name, else the first NVIDIA adapter) and merges it into
/// that adapter's readout. If Windows' counters listed no NVIDIA adapter, the card is added on its own.
fn attach_nvidia(adapters: &mut Vec<Adapter>, nv: Nvidia) {
  let i = adapters
    .iter()
    .position(|a| a.vendor_id == NVIDIA && a.name.eq_ignore_ascii_case(&nv.name))
    .or_else(|| adapters.iter().position(|a| a.vendor_id == NVIDIA));
  let i = i.unwrap_or_else(|| {
    adapters.insert(
      0,
      Adapter {
        luid: String::new(),
        short: short_name(&nv.name),
        name: nv.name.clone(),
        vendor: "NVIDIA".into(),
        integrated: false,
        asleep: false,
        load_pct: nv.load_pct.unwrap_or(0) as f64,
        engines: Vec::new(),
        dedicated_mb: nv.vram_used_mb.unwrap_or(0),
        shared_mb: 0,
        dedicated_total_mb: nv.vram_total_mb.unwrap_or(0),
        readout: None,
        nvidia: None,
        status: Status::default(),
        vendor_id: NVIDIA,
        node_3d: None,
      },
    );
    0
  });
  let a = &mut adapters[i];
  // A sleeping card's NVML answer is ignored, so "Asleep" stays true (the caller does not ask it, this is the backstop).
  if a.asleep {
    return;
  }
  let k = a.readout.as_ref().map(|r| Kmt {
    temp_c: r.temp_c,
    fan_rpm: r.fan_rpm,
    power_pct: r.power_pct,
    clock_mhz: r.clock_mhz,
    max_clock_mhz: r.max_clock_mhz,
    mem_clock_mhz: r.mem_clock_mhz,
    max_mem_clock_mhz: r.max_mem_clock_mhz,
  });
  a.readout = merge(Some(&nv), k.as_ref());
  a.nvidia = Some(nv);
}

/// The verdict for one adapter: "Asleep" for a sleeping card whatever NVML said, else NVML's when it answered, else from the merged readout and Windows' load.
fn adapter_status(a: &Adapter) -> Status {
  let s = |level, text: String| Status { level, text };
  if a.asleep {
    return s("good", "Asleep. Shimmy asks it nothing until an app uses it, so it stays asleep and saves power.".into());
  }
  if let Some(nv) = &a.nvidia {
    return status(nv);
  }
  if let Some(t) = a.readout.as_ref().and_then(|r| r.temp_c) {
    let t = t.round();
    if t >= 90.0 {
      return s("bad", format!("Running hot at {t} °C. Check the vents are clear."));
    }
    if t >= 80.0 {
      return s("warn", format!("Warm at {t} °C. Normal for long gaming or rendering, but worth watching."));
    }
  }
  let load = a.load_pct.round();
  if load < 3.0 {
    return s("good", "Resting. Nothing is keeping it busy.".into());
  }
  s("good", format!("Working at {load}%."))
}

/// The graphics kernel calls. Each query opens a handle to the adapter by its LUID, asks, and always closes it.
mod kmt {
  use super::*;

  pub(super) struct Answer {
    pub adapter_type: Option<u32>,
    pub values: Kmt,
  }

  /// Closes the adapter handle however the query ends.
  struct Handle(u32);
  impl Drop for Handle {
    fn drop(&mut self) {
      let close = D3DKMT_CLOSEADAPTER { hAdapter: self.0 };
      unsafe {
        let _ = D3DKMTCloseAdapter(&close);
      }
    }
  }

  /// One fixed-size answer. `data` is the struct the kernel fills; its input fields must be set by the caller.
  fn ask<T>(h: &Handle, kind: KMTQUERYADAPTERINFOTYPE, data: &mut T) -> bool {
    let mut q = D3DKMT_QUERYADAPTERINFO {
      hAdapter: h.0,
      Type: kind,
      pPrivateDriverData: data as *mut T as *mut core::ffi::c_void,
      PrivateDriverDataSize: std::mem::size_of::<T>() as u32,
    };
    unsafe { D3DKMTQueryAdapterInfo(&mut q) }.is_ok()
  }

  /// None when the adapter cannot be opened (gone, or no such LUID). Fields the driver refuses stay None.
  pub(super) fn query(luid: &str, node: Option<u32>) -> Option<Answer> {
    let (high, low) = luid_parts(luid)?;
    let mut open = D3DKMT_OPENADAPTERFROMLUID { AdapterLuid: LUID { LowPart: low, HighPart: high }, hAdapter: 0 };
    if unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.is_err() {
      return None;
    }
    let h = Handle(open.hAdapter);

    let mut kind = D3DKMT_ADAPTERTYPE::default();
    let adapter_type = ask(&h, KMTQAITYPE_ADAPTERTYPE, &mut kind).then(|| unsafe { kind.Anonymous.Value });

    // Input: physical adapter 0 (laptops have one per LUID). Zeroed by Default.
    let mut perf = D3DKMT_ADAPTER_PERFDATA::default();
    let perf = ask(&h, KMTQAITYPE_ADAPTERPERFDATA, &mut perf).then(|| RawPerf {
      temperature: perf.Temperature,
      fan_rpm: perf.FanRPM,
      power: perf.Power,
      mem_hz: perf.MemoryFrequency,
      max_mem_hz: perf.MaxMemoryFrequency,
    });

    // Input: the 3D engine's node ordinal, physical adapter 0. Not asked when Windows named no 3D engine.
    let n = node_clock(node, |ordinal| {
      let mut np = D3DKMT_NODE_PERFDATA { NodeOrdinal: ordinal, ..Default::default() };
      ask(&h, KMTQAITYPE_NODEPERFDATA, &mut np).then(|| RawNode { hz: np.Frequency, max_hz: np.MaxFrequency })
    });

    Some(Answer { adapter_type, values: clean(perf, n) })
  }
}

/// A read-only look at every adapter the graphics kernel will answer for, for the real-PC check in tests/real_scan.rs.
/// Like the Graphics page, it skips a dedicated card that Windows shows idle, so it never wakes one.
pub fn kmt_report(c: &mut Counters) -> Vec<(String, bool, Option<u32>, Option<Kmt>)> {
  let (adapters, _) = c.sample(&HashMap::new());
  adapters
    .into_iter()
    .map(|a| {
      let k = if a.asleep { None } else { kmt::query(&a.luid, a.node_3d) };
      (a.name, a.asleep, k.as_ref().and_then(|k| k.adapter_type), k.map(|k| k.values))
    })
    .collect()
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
      (dgpu.clone(), AdapterInfo { name: "NVIDIA GeForce RTX 3050 Laptop GPU".into(), vendor_id: NVIDIA, dedicated_total: 4 << 30, kmt_type: None }),
      (igpu.clone(), AdapterInfo { name: "AMD Radeon(TM) Graphics".into(), vendor_id: AMD, dedicated_total: 512 << 20, kmt_type: None }),
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
    // The RTX is in use, so it may be asked; its clock is read from the engine Windows calls 3D.
    assert!(!adapters[0].asleep);
    assert_eq!((adapters[0].vendor.as_str(), adapters[1].vendor.as_str()), ("NVIDIA", "AMD"));
    assert_eq!(adapters[0].node_3d, Some(0));

    // An idle RTX next to a built-in chip is asleep, so nothing is asked of it.
    let idle = vec![e(12, "BBBB", 0, "3D", 8.0), e(12, "AAAA", 0, "3D", 0.2)];
    let (adapters, _) = summarise(&idle, &[], &[], &info, &names);
    assert!(adapters[0].asleep && !adapters[0].integrated);
    assert!(!adapters[1].asleep);
  }

  #[test]
  fn luid_round_trip() {
    let key = luid_of("pid_4_luid_0x00000001_0x0000D1B5_phys_0_eng_0_engtype_3D").unwrap();
    assert_eq!(key, "000000010000D1B5");
    assert_eq!(luid_parts(&key), Some((1, 0xD1B5)));
    assert_eq!(luid_parts("FFFFFFFF00000002"), Some((-1, 2)));
    assert_eq!(luid_parts("D1B5"), None);
    assert_eq!(luid_parts("zzzzzzzzzzzzzzzz"), None);
    let e = "pid_9_luid_0x00000000_0x0000AAAA_phys_0_eng_7_engtype_3D";
    assert_eq!(parse_engine(e).map(|p| p.2), Some(7));
  }

  #[test]
  fn integrated_or_dedicated() {
    // Before the kernel is asked: NVIDIA is always dedicated; others by their own memory.
    assert!(!is_integrated(NVIDIA, 256 << 20, None));
    assert!(is_integrated(AMD, 512 << 20, None));
    assert!(is_integrated(INTEL, 128 << 20, None));
    assert!(!is_integrated(AMD, 8 << 30, None));
    // A Radeon chip with a 2 GB carve-out starts as dedicated (the safe side), then the kernel's hybrid bit corrects it.
    assert!(!is_integrated(AMD, 2 << 30, None));
    assert!(is_integrated(AMD, 2 << 30, Some(HYBRID_INTEGRATED | 1)));
    assert!(!is_integrated(INTEL, 128 << 20, Some(HYBRID_DISCRETE)));
    // No hybrid bits (a desktop): memory decides.
    assert!(is_integrated(INTEL, 128 << 20, Some(1)));
    // Sleep: only a dedicated card, idle, next to a built-in chip.
    assert!(may_sleep(false, 0.4, true));
    assert!(!may_sleep(false, 0.4, false));
    assert!(!may_sleep(false, 12.0, true));
    assert!(!may_sleep(true, 0.0, true));
  }

  /// A hybrid laptop: the RTX plus an AMD or Intel chip, idle or busy.
  fn hybrid(builtin_vendor: u32, builtin_bytes: u64, kmt_type: Option<u32>, rtx_busy: bool) -> Vec<Adapter> {
    let info = HashMap::from([
      ("000000000000AAAA".to_string(), AdapterInfo { name: "NVIDIA GeForce RTX 3050 Laptop GPU".into(), vendor_id: NVIDIA, dedicated_total: 4 << 30, kmt_type: None }),
      ("000000000000BBBB".to_string(), AdapterInfo { name: "Built-in chip".into(), vendor_id: builtin_vendor, dedicated_total: builtin_bytes, kmt_type }),
    ]);
    let e = |l: &str, v: f64| (format!("pid_1_luid_0x00000000_0x0000{l}_phys_0_eng_0_engtype_3D"), v);
    let engines = vec![e("BBBB", 8.0), e("AAAA", if rtx_busy { 30.0 } else { 0.0 })];
    summarise(&engines, &[], &[], &info, &HashMap::new()).0
  }

  #[test]
  fn sleep_on_a_hybrid_laptop_with_a_big_builtin_carve_out() {
    // The AMD or Intel chip reports 2 GB, so by memory alone it looks dedicated. Next to an NVIDIA card it is still the
    // built-in one, so the idle RTX sleeps and the Battery page's pick of dedicated card is the RTX.
    for (vendor, kmt) in [(AMD, None), (INTEL, None), (AMD, Some(HYBRID_INTEGRATED))] {
      let a = hybrid(vendor, 2 << 30, kmt, false);
      let rtx = a.iter().find(|x| x.vendor_id == NVIDIA).unwrap();
      let chip = a.iter().find(|x| x.vendor_id == vendor).unwrap();
      assert!(chip.integrated && !chip.asleep, "{vendor:#x} {kmt:?}");
      assert!(!rtx.integrated && rtx.asleep, "{vendor:#x} {kmt:?}");
    }
    // In use, the RTX is awake.
    assert!(!hybrid(AMD, 2 << 30, None, true).iter().any(|x| x.asleep));
    // The kernel saying the chip is the discrete one wins.
    assert!(!is_builtin(AMD, 2 << 30, Some(HYBRID_DISCRETE), true));
    // No NVIDIA card: memory decides as before.
    assert!(!is_builtin(AMD, 2 << 30, None, false));
    assert!(is_builtin(AMD, 512 << 20, None, false));
  }

  #[test]
  fn nvml_only_for_an_awake_nvidia_card() {
    // Asleep: not asked.
    assert!(!nvml_wanted(&hybrid(AMD, 2 << 30, None, false)));
    // Awake: asked.
    assert!(nvml_wanted(&hybrid(AMD, 512 << 20, None, true)));
    // No NVIDIA chip, or no list at all (Windows' counters failed): never asked blindly.
    let info = HashMap::from([("000000000000BBBB".to_string(), AdapterInfo { name: "Intel(R) Iris(R) Xe Graphics".into(), vendor_id: INTEL, dedicated_total: 128 << 20, kmt_type: None })]);
    assert!(!nvml_wanted(&summarise(&[], &[], &[], &info, &HashMap::new()).0));
    assert!(!nvml_wanted(&[]));
    // A lone NVIDIA card runs the screen, so it is awake and asked.
    let lone = HashMap::from([("000000000000AAAA".to_string(), AdapterInfo { name: "NVIDIA GeForce RTX 3050".into(), vendor_id: NVIDIA, dedicated_total: 4 << 30, kmt_type: None })]);
    assert!(nvml_wanted(&summarise(&[], &[], &[], &lone, &HashMap::new()).0));
  }

  #[test]
  fn a_sleeping_card_ignores_nvml() {
    let mut a = hybrid(AMD, 2 << 30, None, false);
    let nv = Nvidia { name: "NVIDIA GeForce RTX 3050 Laptop GPU".into(), temp_c: Some(46), load_pct: Some(0), ..Default::default() };
    readouts_without_counters(&mut a, Some(nv.clone()));
    let rtx = a.iter().find(|x| x.vendor_id == NVIDIA).unwrap();
    assert!(rtx.asleep && rtx.nvidia.is_none() && rtx.readout.is_none());
    assert!(rtx.status.text.starts_with("Asleep"));
    // Even a stray NVML answer on a sleeping card does not change its verdict.
    let mut stray = rtx.clone();
    stray.nvidia = Some(nv.clone());
    assert!(adapter_status(&stray).text.starts_with("Asleep"));
    // Awake, NVML is attached.
    let mut b = hybrid(AMD, 2 << 30, None, true);
    readouts_without_counters(&mut b, Some(nv));
    assert!(b.iter().find(|x| x.vendor_id == NVIDIA).unwrap().nvidia.is_some());
  }

  #[test]
  fn no_3d_engine_means_no_clock() {
    // Only Copy runs on the RTX: Windows named no 3D engine, so node 0 is not assumed to be it.
    let info = HashMap::from([("000000000000AAAA".to_string(), AdapterInfo { name: "NVIDIA GeForce RTX 3050".into(), vendor_id: NVIDIA, dedicated_total: 4 << 30, kmt_type: None })]);
    let copy = vec![("pid_1_luid_0x00000000_0x0000AAAA_phys_0_eng_2_engtype_Copy".to_string(), 5.0)];
    let a = summarise(&copy, &[], &[], &info, &HashMap::new()).0;
    assert_eq!(a[0].node_3d, None);
    let mut asked = false;
    let n = node_clock(a[0].node_3d, |_| {
      asked = true;
      Some(RawNode { hz: 400_000_000, max_hz: 2_100_000_000 })
    });
    assert!(!asked && n.is_none());
    assert_eq!(clean(None, n).clock_mhz, None);
    // With a 3D engine the node is asked for.
    let n = node_clock(Some(3), |o| (o == 3).then_some(RawNode { hz: 400_000_000, max_hz: 2_100_000_000 }));
    assert_eq!(clean(None, n).clock_mhz, Some(400));
  }

  #[test]
  fn zero_means_not_reported() {
    let k = clean(Some(RawPerf { temperature: 0, fan_rpm: 0, power: 0, mem_hz: 0, max_mem_hz: 0 }), Some(RawNode { hz: 0, max_hz: 0 }));
    assert_eq!(k, Kmt::default());
    let k = clean(
      Some(RawPerf { temperature: 523, fan_rpm: 2100, power: 125, mem_hz: 1_750_000_000, max_mem_hz: 1_750_000_000 }),
      Some(RawNode { hz: 400_000_000, max_hz: 2_100_000_000 }),
    );
    assert_eq!(k.temp_c, Some(52.3));
    assert_eq!(k.fan_rpm, Some(2100));
    assert_eq!(k.power_pct, Some(12.5));
    assert_eq!((k.clock_mhz, k.max_clock_mhz, k.mem_clock_mhz), (Some(400), Some(2100), Some(1750)));
    // Nonsense is dropped the same way.
    let k = clean(Some(RawPerf { temperature: 60_000, fan_rpm: 900_000, ..Default::default() }), None);
    assert_eq!((k.temp_c, k.fan_rpm, k.clock_mhz), (None, None, None));
    assert_eq!(clean(None, None), Kmt::default());
  }

  #[test]
  fn nvml_wins_kernel_fills() {
    let k = Kmt { temp_c: Some(51.0), fan_rpm: Some(1800), power_pct: Some(20.0), clock_mhz: Some(300), max_clock_mhz: Some(1700), ..Default::default() };
    let nv = Nvidia { temp_c: Some(49), power_w: Some(6.5), clock_mhz: Some(210), ..Default::default() };
    let r = merge(Some(&nv), Some(&k)).unwrap();
    assert_eq!(r.temp_c, Some(49.0));
    assert_eq!(r.clock_mhz, Some(210));
    assert_eq!(r.max_clock_mhz, Some(1700));
    assert_eq!(r.fan_rpm, Some(1800));
    // Watts from NVML replace the kernel's share of the power budget.
    assert_eq!((r.power_w, r.power_pct), (Some(6.5), None));
    let r = merge(None, Some(&k)).unwrap();
    assert_eq!((r.power_w, r.power_pct), (None, Some(20.0)));
    assert_eq!(merge(None, Some(&Kmt::default())), None);
    assert_eq!(merge(None, None), None);
  }

  #[test]
  fn nvml_goes_on_the_nvidia_adapter() {
    let info = HashMap::from([
      ("000000000000AAAA".to_string(), AdapterInfo { name: "NVIDIA GeForce RTX 3050 Laptop GPU".into(), vendor_id: NVIDIA, dedicated_total: 4 << 30, kmt_type: None }),
      ("000000000000BBBB".to_string(), AdapterInfo { name: "AMD Radeon(TM) Graphics".into(), vendor_id: AMD, dedicated_total: 512 << 20, kmt_type: None }),
    ]);
    // The RTX is in use (an asleep card takes no NVML answer).
    let busy = vec![("pid_1_luid_0x00000000_0x0000AAAA_phys_0_eng_0_engtype_3D".to_string(), 20.0)];
    let (mut adapters, _) = summarise(&busy, &[], &[], &info, &HashMap::new());
    let nv = Nvidia { name: "NVIDIA GeForce RTX 3050 Laptop GPU".into(), temp_c: Some(46), pstate: Some(8), load_pct: Some(0), ..Default::default() };
    attach_nvidia(&mut adapters, nv.clone());
    assert_eq!(adapters.len(), 2);
    let i = adapters.iter().position(|a| a.vendor == "NVIDIA").unwrap();
    assert_eq!(adapters[i].readout.as_ref().unwrap().temp_c, Some(46.0));
    assert!(adapters.iter().filter(|a| a.vendor != "NVIDIA").all(|a| a.nvidia.is_none()));
    // No NVIDIA adapter in Windows' list: the card is added on its own.
    let mut none: Vec<Adapter> = Vec::new();
    attach_nvidia(&mut none, nv);
    assert_eq!((none.len(), none[0].short.as_str()), (1, "RTX 3050"));
  }

  #[test]
  fn verdicts_for_any_vendor() {
    let info = HashMap::from([("000000000000BBBB".to_string(), AdapterInfo { name: "Intel(R) Iris(R) Xe Graphics".into(), vendor_id: INTEL, dedicated_total: 128 << 20, kmt_type: None })]);
    let (mut a, _) = summarise(&[], &[], &[], &info, &HashMap::new());
    assert_eq!(adapter_status(&a[0]).text, "Resting. Nothing is keeping it busy.");
    a[0].readout = merge(None, Some(&Kmt { temp_c: Some(91.0), ..Default::default() }));
    assert_eq!(adapter_status(&a[0]).level, "bad");
    a[0].asleep = true;
    assert!(adapter_status(&a[0]).text.starts_with("Asleep"));
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
