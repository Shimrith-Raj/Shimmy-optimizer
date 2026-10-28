pub mod asus;
pub mod auto;
pub mod autostart;
pub mod battery;
pub mod caches;
pub mod clean;
pub mod cpu;
pub mod finder;
pub mod gpu;
pub mod heat;
pub mod hold;
pub mod jobs;
pub mod procs;
pub mod sensors;
pub mod storage;
pub mod startup;
pub mod store;
pub mod system;
pub mod tray;

use procs::{build_groups, classify, recommend};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;
use store::Store;
use sysinfo::System;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent, Wry};
use tauri_plugin_notification::NotificationExt;

pub struct Shared {
  sys: Mutex<System>,
  store: Store,
  /// One close/trim/clean at a time.
  busy: AtomicBool,
  sensors: sensors::SensorState,
  atk: Option<asus::Atk>,
  /// Opened on first use, only when the CPU page asks for the GPU.
  nvml: Mutex<Option<Result<nvml_wrapper::Nvml, String>>>,
  base_mhz: Option<f64>,
  profile: PathBuf,
  /// The finder's standard folders: Downloads, Desktop, Documents, Videos, Pictures.
  default_roots: Vec<PathBuf>,
  /// Windows' GPU counters, opened when a page first asks for the GPU.
  gpu_counters: Mutex<Option<Result<gpu::Counters, String>>>,
  watch: Mutex<auto::Watch>,
  hold_watch: Mutex<hold::Watch>,
  heat_watch: Mutex<heat::Watch>,
  /// Only the first failure to turn boost off is logged, so a refusing powercfg does not fill the log every 30 s.
  heat_failed: AtomicBool,
  /// The last hour of battery and processor watts, sampled every 3 s, in memory only.
  power_hist: Mutex<VecDeque<battery::PowerPoint>>,
  /// What the rest of the laptop draws, learned on battery; used to estimate the total on the charger.
  rest_w: Mutex<Option<f64>>,
  usage: jobs::Slot<storage::Usage>,
  tree: Mutex<Option<storage::Tree>>,
  clean_scan: jobs::Slot<Vec<clean::CatScan>>,
  clean_run: jobs::Slot<Vec<clean::CatResult>>,
  dism: jobs::Slot<clean::Dism>,
  finder: jobs::Slot<finder::Found>,
  cache_scan: jobs::Slot<Vec<caches::CacheScan>>,
  cache_run: jobs::Slot<Vec<caches::CacheResult>>,
  /// Windows' battery report, refreshed at most every 10 minutes (it takes about two seconds to make).
  battery_report: Mutex<Option<(u64, battery::Report)>>,
  /// When making the report last failed, so a failing powercfg is not re-run (and re-reported) every 5 s.
  battery_report_failed: Mutex<u64>,
  settings: Mutex<Settings>,
  settings_path: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
  /// Battery charge limit chosen in Shimmy. The firmware cannot report it back, so Shimmy remembers and re-applies it.
  charge_limit: Option<u32>,
  alerts: auto::Alerts,
  auto_clean: auto::AutoClean,
  weekly: auto::Weekly,
  finder: auto::FinderPrefs,
  /// Run from the charger (hold.rs).
  plugged: hold::PluggedMode,
  /// Old caches on the Storage page.
  caches: caches::CachePrefs,
  /// Heat protection: CPU boost off while the processor is hot (heat.rs).
  heat: heat::HeatPrefs,
}

impl Shared {
  /// Fresh sensor readings, or None when the sidecar is not delivering (not elevated, driver missing).
  fn readings(&self) -> (Option<Vec<sensors::Reading>>, Option<String>) {
    let l = self.sensors.lock().unwrap();
    let fresh = l.at > 0 && store::now_ms().saturating_sub(l.at) < sensors::STALE_MS;
    (fresh.then(|| l.rows.clone()), l.error.clone())
  }

  fn save_settings(&self) {
    let json = serde_json::to_string_pretty(&*self.settings.lock().unwrap()).unwrap_or_default();
    let _ = std::fs::write(&self.settings_path, json);
  }

  fn limit_supported(&self) -> bool {
    self.atk.as_ref().and_then(|a| a.get(asus::CHARGE_LIMIT)).is_some_and(|raw| raw & 0x0001_0000 != 0)
  }

  /// Writes the remembered limit to the firmware again (MyASUS can change it behind Shimmy's back): the hold level while
  /// Run from the charger is on, otherwise the 60/80/100 choice.
  fn reapply_charge_limit(&self) {
    let limit = {
      let st = self.settings.lock().unwrap();
      hold::effective_limit(&st.plugged, st.charge_limit)
    };
    if let Some(limit) = limit {
      let _ = self.write_limit(limit);
    }
  }

  fn write_limit(&self, pct: u32) -> Result<u32, String> {
    let atk = self.atk.as_ref().ok_or("The ASUS control interface is not available.")?;
    // ASUS firmware answers 1 when it takes the value (logged on this laptop); anything else is a refusal.
    match atk.set(asus::CHARGE_LIMIT, pct) {
      Some(1) => Ok(1),
      Some(r) => Err(format!("The firmware refused the {pct}% charge limit (it answered {r}).")),
      None => Err("The firmware did not accept the request.".to_string()),
    }
  }

  /// Plugged in, charge and charge rate, from the sensors when they deliver and from Windows otherwise. None when
  /// neither knows the charge, so a hold is never set from a made-up 0 %.
  fn battery_reading(&self) -> Option<hold::Reading> {
    let live = system::battery_now();
    let rows = self.readings().0;
    let v = |t: &str, n: &str| rows.as_deref().and_then(|r| sensors::value(r, "Battery", t, n));
    Some(hold::Reading {
      plugged: live.plugged,
      charge_pct: v("Level", "Charge Level").or(live.pct.map(f64::from))?,
      rate_w: rows.as_deref().and_then(battery::rate),
    })
  }

  /// One power sample for the Battery page's last-hour chart, and the rest-of-laptop draw learned while on battery.
  fn sample_power(&self) {
    let live = system::battery_now();
    if !live.present {
      return;
    }
    let rows = self.readings().0;
    let battery_w = rows.as_deref().and_then(battery::rate);
    let cpu_w = rows.as_deref().and_then(|r| sensors::value(r, "Cpu", "Power", "Package"));
    if !live.plugged {
      if let Some(rest) = battery_w.zip(cpu_w).and_then(|(b, c)| battery::rest_of(b, c)) {
        let mut r = self.rest_w.lock().unwrap();
        *r = Some(r.map_or(rest, |old| old * 0.9 + rest * 0.1));
      }
    }
    let held = self.settings.lock().unwrap().plugged.on;
    let mut h = self.power_hist.lock().unwrap();
    if h.len() >= battery::HIST_POINTS {
      h.pop_front();
    }
    h.push_back(battery::PowerPoint { t: store::now_ms(), battery_w, cpu_w, plugged: live.plugged, held });
  }

  /// Whether there is a dedicated graphics card, whether it is awake (Windows' GPU engine counters show it in use) and,
  /// only then, its watts from NVML. Asking NVML wakes a sleeping card, so it is never asked while the card sleeps.
  fn rtx_power(&self) -> (bool, bool, Option<f64>) {
    let (present, awake) = match self.gpu_counters.lock().unwrap().get_or_insert_with(gpu::Counters::open) {
      Ok(c) => {
        let adapters = c.sample(&HashMap::new()).0;
        let dedicated: Vec<_> = adapters.iter().filter(|a| !a.integrated).collect();
        (!dedicated.is_empty(), dedicated.iter().any(|a| a.load_pct >= 1.0))
      }
      Err(_) => (false, false),
    };
    if !awake {
      return (present, false, None);
    }
    let w = match self.nvml.lock().unwrap().get_or_insert_with(|| nvml_wrapper::Nvml::init().map_err(|e| e.to_string())) {
      Ok(n) => gpu::nvidia(n).ok().and_then(|g| g.power_w),
      Err(_) => None,
    };
    (true, true, w)
  }

  /// Puts CPU boost back if Shimmy turned it off. Used when heat protection is switched off, at Exit and at start-up.
  fn restore_boost(&self, why: &str) {
    let Some(saved) = self.settings.lock().unwrap().heat.saved.clone() else { return };
    match heat::restore(&saved) {
      Ok(()) => {
        self.settings.lock().unwrap().heat.saved = None;
        self.save_settings();
        self.store.log("boost-on", &format!("AC {}, DC {}", saved.ac, saved.dc), why);
      }
      Err(e) => self.store.log("boost-on-failed", &format!("AC {}, DC {}", saved.ac, saved.dc), &e),
    }
  }

  /// The standard folders the user has not turned off, then the ones they added.
  fn finder_roots(&self) -> Vec<PathBuf> {
    let f = self.settings.lock().unwrap().finder.clone();
    self.default_roots
      .iter()
      .filter(|r| !f.skip_roots.iter().any(|s| Path::new(s) == r.as_path()))
      .cloned()
      .chain(f.extra_roots.iter().map(PathBuf::from).filter(|p| p.is_dir()))
      .collect()
  }

  fn finder_rules(&self) -> finder::Rules {
    let f = &self.settings.lock().unwrap().finder;
    finder::Rules::new(f.large_mb, &f.exclusions)
  }

  /// Frees what only the window needs once it is closed: scan results, the C: tree, the battery report, the GPU
  /// libraries and the full process list. Then the freed heap is handed back to Windows.
  fn release(&self) {
    *self.tree.lock().unwrap() = None;
    jobs::clear(&self.usage);
    jobs::clear(&self.clean_scan);
    jobs::clear(&self.clean_run);
    jobs::clear(&self.dism);
    jobs::clear(&self.finder);
    jobs::clear(&self.cache_scan);
    jobs::clear(&self.cache_run);
    *self.battery_report.lock().unwrap() = None;
    *self.nvml.lock().unwrap() = None;
    *self.gpu_counters.lock().unwrap() = None;
    {
      let mut sys = self.sys.lock().unwrap();
      *sys = System::new();
      sys.refresh_cpu_all();
    }
    system::compact_self();
  }

  /// Lower-case names of running processes, fresh.
  fn running(&self) -> HashSet<String> {
    let mut sys = self.sys.lock().unwrap();
    system::list_procs(&mut sys).into_iter().map(|p| p.name.to_lowercase()).collect()
  }

  fn fans(&self) -> Vec<cpu::Fan> {
    let Some(atk) = &self.atk else { return Vec::new() };
    [("Processor fan", asus::CPU_FAN), ("Graphics fan", asus::GPU_FAN)]
      .into_iter()
      .filter_map(|(name, id)| atk.fan_rpm(id).map(|rpm| cpu::Fan { name: name.into(), rpm }))
      .collect()
  }
}

type Shr = Arc<Shared>;

struct TrayStatus(MenuItem<Wry>);
struct TrayPlugged(CheckMenuItem<Wry>);

fn mb_of(bytes: i64) -> i64 {
  (bytes as f64 / 1_048_576.0).round() as i64
}

fn free_bytes() -> i64 {
  let m = system::mem_now();
  m.free_mb as i64 * 1_048_576
}

fn fmt_mb(mb: i64) -> String {
  if mb.abs() >= 1024 { format!("{:.1} GB", mb as f64 / 1024.0) } else { format!("{mb} MB") }
}

/// Runs blocking work off the main thread, one destructive action at a time.
async fn exclusive<T: Send + 'static>(s: &Shr, f: impl FnOnce(&Shared) -> Result<T, String> + Send + 'static) -> Result<T, String> {
  if s.busy.swap(true, Ordering::SeqCst) {
    return Err("Another clean-up is still running.".into());
  }
  let s2 = s.clone();
  let r = tauri::async_runtime::spawn_blocking(move || f(&s2)).await.map_err(|e| e.to_string());
  s.busy.store(false, Ordering::SeqCst);
  r?
}

fn close_impl(s: &Shared, targets: Vec<procs::Proc>, force: bool) -> Value {
  let before = free_bytes();
  let pids: Vec<u32> = targets.iter().map(|p| p.pid).collect();
  let remaining = system::close(&pids, force);
  std::thread::sleep(Duration::from_millis(800));
  let freed = mb_of(free_bytes() - before).max(0);
  let mut apps: Vec<String> = Vec::new();
  for p in &targets {
    let a = procs::app_of(p);
    if !apps.contains(&a) {
      apps.push(a);
    }
  }
  let closed = targets.len() - remaining.len();
  s.store.log(
    if force { "force-close" } else { "close" },
    &apps.join("; "),
    &format!("{closed}/{} closed, ~{freed} MB freed; pids {}", targets.len(),
      targets.iter().map(|p| format!("{}:{}", p.pid, p.name)).collect::<Vec<_>>().join(" ")),
  );
  json!({
    "closed": closed,
    "remaining": remaining.iter().map(|pid| json!({ "pid": pid, "name": targets.iter().find(|p| p.pid == *pid).map(|p| p.name.clone()) })).collect::<Vec<_>>(),
    "freedMB": freed,
  })
}

fn trim_impl(s: &Shared) -> Value {
  let before = free_bytes();
  let n = system::trim_all(&s.sys.lock().unwrap());
  std::thread::sleep(Duration::from_millis(1000));
  let freed = mb_of(free_bytes() - before);
  s.store.log("trim-working-sets", "all processes", &format!("{n} processes trimmed, {freed} MB change in available memory"));
  json!({ "trimmed": n, "freedMB": freed })
}

/// Closes only "safe" suggestions whose processes are all windowless, then trims. Shared by the button and the tray.
fn quick_clean_impl(s: &Shared) -> Value {
  let procs_now = system::list_procs(&mut s.sys.lock().unwrap());
  let own = system::own_pids(&procs_now);
  let groups = build_groups(&procs_now, &own);
  let before = free_bytes();
  let (mut targets, mut closed_apps, mut skipped_apps) = (Vec::new(), Vec::new(), Vec::new());
  for r in recommend(&procs_now, &groups, &system::mem_now()) {
    if r.action != "close" || r.risk != "low" {
      continue;
    }
    let list: Vec<procs::Proc> = procs_now.iter().filter(|p| r.pids.contains(&p.pid) && !classify(p, &own).protected).cloned().collect();
    if list.is_empty() {
      continue;
    }
    let app = r.app.unwrap_or(r.title);
    if list.iter().any(|p| !p.title.is_empty()) {
      skipped_apps.push(app);
      continue;
    }
    targets.extend(list);
    closed_apps.push(app);
  }
  let closed = if targets.is_empty() { json!({ "closed": 0 }) } else { close_impl(s, targets, true) };
  let trim = trim_impl(s);
  json!({
    "closedApps": closed_apps, "skippedApps": skipped_apps, "closed": closed["closed"], "trimmed": trim["trimmed"],
    "freedMB": mb_of(free_bytes() - before), "mem": system::mem_now(),
  })
}

// ---------- Commands ----------

#[tauri::command]
async fn get_state(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let mut sys = s.sys.lock().unwrap();
    let procs_now = system::list_procs(&mut sys);
    let own = system::own_pids(&procs_now);
    let groups = build_groups(&procs_now, &own);
    let mem = system::mem_now();
    let recs = recommend(&procs_now, &groups, &mem);
    let home = system::home(&mut sys);
    let loads: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
    let name = sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
    drop(sys);
    let (rows, err) = s.readings();
    let cpu = cpu::build(name, &loads, home.cpu_pct, s.base_mhz, rows.as_deref(), s.fans(), err);
    let battery_health = {
      let report = s.battery_report.lock().unwrap();
      battery::health(rows.as_deref(), report.as_ref().map(|(_, r)| r)).map(f64::round)
    };
    let plugged = s.settings.lock().unwrap().plugged.clone();
    // Shimmy itself: this process, its window engine and the sensor sidecar.
    let self_mb = mb_of(procs_now.iter().filter(|p| own.contains(&p.pid)).map(|p| p.ws as i64).sum());
    let heat = s.settings.lock().unwrap().heat.clone();
    let heat = json!({ "enabled": heat.enabled, "boostOff": heat.saved.is_some(), "since": heat.saved.map(|b| b.since), "hotC": heat::HOT_C, "coolC": heat::COOL_C });
    let fan_mode = s.atk.as_ref().and_then(|a| a.fan_mode());
    json!({ "mem": mem, "groups": groups, "recs": recs, "home": home, "cpu": cpu, "batteryHealth": battery_health, "plugged": plugged, "selfMB": self_mb, "fanMode": fan_mode, "heat": heat, "at": store::now_ms() })
  })
  .await
  .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_history(s: State<'_, Shr>) -> Value {
  json!({ "points": s.store.history(), "cpu": s.store.cpu_history() })
}

/// The Graphics page (and the Processor page's summary): NVIDIA detail through NVML, which wakes the RTX, so it is
/// read only while a page asks; plus Windows' counters for every GPU, per engine and per app.
#[tauri::command]
async fn get_gpu(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let (nv, error) = {
      let mut slot = s.nvml.lock().unwrap();
      match slot.get_or_insert_with(|| nvml_wrapper::Nvml::init().map_err(|e| e.to_string())) {
        Ok(n) => match gpu::nvidia(n) {
          Ok(g) => (Some(g), None),
          Err(e) => (None, Some(e)),
        },
        Err(e) => (None, Some(e.clone())),
      }
    };
    let names: HashMap<u32, String> = system::list_procs(&mut s.sys.lock().unwrap()).iter().map(|p| (p.pid, procs::app_of(p))).collect();
    let mut slot = s.gpu_counters.lock().unwrap();
    let (adapters, apps, counters_error) = match slot.get_or_insert_with(gpu::Counters::open) {
      Ok(c) => {
        let (a, p) = c.sample(&names);
        (a, p, None)
      }
      Err(e) => (Vec::new(), Vec::new(), Some(e.clone())),
    };
    json!({ "gpu": nv, "error": error, "adapters": adapters, "apps": apps, "countersError": counters_error })
  })
  .await
  .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_log(s: State<'_, Shr>) -> Value {
  json!({ "rows": s.store.read_log() })
}

#[tauri::command]
async fn get_startup() -> Result<Value, String> {
  tauri::async_runtime::spawn_blocking(|| json!({ "items": startup::list() })).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn toggle_startup(s: State<'_, Shr>, name: String, sub: String, scope: String, enable: bool) -> Result<Value, String> {
  let entry = startup::list().into_iter().find(|e| e.name == name && e.sub == sub && e.scope == scope).ok_or("Startup entry not found.")?;
  startup::toggle(&name, &sub, &scope, enable)?;
  s.store.log(if enable { "startup-enable" } else { "startup-disable" }, &name, &entry.command);
  Ok(json!({ "ok": true }))
}

#[tauri::command]
async fn close_procs(s: State<'_, Shr>, pids: Vec<u32>, mode: String) -> Result<Value, String> {
  exclusive(s.inner(), move |s| {
    let mut want: Vec<u32> = pids.into_iter().filter(|p| *p > 4).collect();
    want.dedup();
    want.truncate(500);
    let procs_now = system::list_procs(&mut s.sys.lock().unwrap());
    let own = system::own_pids(&procs_now);
    let (mut targets, mut skipped) = (Vec::new(), Vec::new());
    for pid in &want {
      match procs_now.iter().find(|p| p.pid == *pid) {
        None => skipped.push(json!({ "pid": pid, "reason": "already gone" })),
        Some(p) => {
          let c = classify(p, &own);
          if c.protected {
            skipped.push(json!({ "pid": pid, "name": p.name, "reason": format!("protected ({})", c.reason.unwrap_or_default()) }));
          } else {
            targets.push(p.clone());
          }
        }
      }
    }
    if targets.is_empty() {
      return Ok(json!({ "requested": want.len(), "closed": 0, "remaining": [], "skipped": skipped, "freedMB": 0 }));
    }
    let force = mode == "force" || targets.iter().all(|p| p.title.is_empty());
    let mut r = close_impl(s, targets, force);
    r["requested"] = json!(want.len());
    r["skipped"] = json!(skipped);
    r["mode"] = json!(if force { "force" } else { "graceful" });
    Ok(r)
  })
  .await
}

#[tauri::command]
async fn trim(s: State<'_, Shr>) -> Result<Value, String> {
  exclusive(s.inner(), |s| Ok(trim_impl(s))).await
}

#[tauri::command]
async fn quick_clean(s: State<'_, Shr>) -> Result<Value, String> {
  exclusive(s.inner(), |s| Ok(quick_clean_impl(s))).await
}

// ---------- Storage & Clean ----------

fn job_json<T: serde::Serialize>(slot: &jobs::Slot<T>) -> Value {
  serde_json::to_value(&*slot.lock().unwrap()).unwrap_or(Value::Null)
}

#[tauri::command]
async fn get_storage(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let health = s.readings().0.map(|r| storage::health(&r)).unwrap_or_default();
    let (bin_bytes, bin_items) = clean::recycle_bin();
    let age = s.settings.lock().unwrap().caches.age_days;
    json!({
      "drives": storage::drives(), "health": health, "usage": job_json(&s.usage), "profile": s.profile,
      "caches": job_json(&s.cache_scan), "cacheRun": job_json(&s.cache_run), "cacheAgeDays": age,
      "bin": { "bytes": bin_bytes, "items": bin_items, "capacity": finder::bin_capacity(&s.profile) },
      "hiberBytes": storage::hiberfil_bytes(),
    })
  })
  .await
  .map_err(|e| e.to_string())
}

#[tauri::command]
fn start_usage_scan(s: State<'_, Shr>) -> bool {
  let s2 = s.inner().clone();
  jobs::start(&s.usage, move |progress| {
    let (usage, tree) = storage::scan_usage(s2.profile.clone(), progress);
    *s2.tree.lock().unwrap() = Some(tree);
    Ok(usage)
  })
}

#[tauri::command]
fn start_cache_scan(s: State<'_, Shr>) -> bool {
  let s2 = s.inner().clone();
  jobs::start(&s.cache_scan, move |progress| {
    let prefs = s2.settings.lock().unwrap().caches.clone();
    Ok(caches::scan(&prefs, &s2.running(), &progress))
  })
}

/// Moves the old files of the chosen caches to the Recycle Bin, then measures again.
#[tauri::command]
fn start_cache_clean(s: State<'_, Shr>, ids: Vec<String>) -> bool {
  let s2 = s.inner().clone();
  let ids: Vec<String> = ids.into_iter().filter(|i| caches::is_cache(i)).collect();
  jobs::start(&s.cache_run, move |progress| {
    let prefs = s2.settings.lock().unwrap().caches.clone();
    let results = caches::run(&ids, &prefs, &s2.running(), &progress);
    for r in &results {
      s2.store.log("recycle-cache", r.title, &format!("{} files older than {} days, {} MB, moved to the Recycle Bin{}", r.files, prefs.age_days,
        r.recycled / 1_048_576, r.note.as_ref().map(|n| format!("; {n}")).unwrap_or_default()));
    }
    *s2.cache_scan.lock().unwrap() = jobs::Job { result: Some(caches::scan(&prefs, &s2.running(), &progress)), at: store::now_ms(), ..Default::default() };
    Ok(results)
  })
}

/// What deleting this folder would remove, or why it is refused.
#[tauri::command]
async fn folder_preview(s: State<'_, Shr>, path: String) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let p = PathBuf::from(&path);
    let bin = finder::bin_capacity(&p);
    match storage::check_folder(&p, &s.profile, &s.finder_rules()) {
      Ok(c) => json!({ "path": path, "contents": c, "binCapacity": bin, "binBytes": clean::recycle_bin().0 }),
      Err(why) => json!({ "path": path, "refused": why }),
    }
  })
  .await
  .map_err(|e| e.to_string())
}

/// Deletes a folder from the list: to the Recycle Bin, or for good when `permanent`. The gate is checked again first.
/// A folder too big for the bin would be deleted for good by Windows, so that needs `allow_big` from a second question.
#[tauri::command]
async fn folder_delete(s: State<'_, Shr>, path: String, permanent: bool, allow_big: bool) -> Result<Value, String> {
  exclusive(s.inner(), move |s| {
    let p = PathBuf::from(&path);
    let c = storage::check_folder(&p, &s.profile, &s.finder_rules()).map_err(|why| format!("Not deleted: {why}."))?;
    if !permanent && c.bytes > finder::bin_capacity(&p) && !allow_big {
      return Ok(json!({ "needsConfirm": true, "bytes": c.bytes }));
    }
    let gone_for_good = permanent || c.bytes > finder::bin_capacity(&p);
    if permanent {
      std::fs::remove_dir_all(&p).map_err(|e| format!("Windows could not delete it: {e}"))?;
    } else {
      trash::delete(&p).map_err(|e| format!("Windows could not move it to the Recycle Bin: {e}"))?;
    }
    s.store.log(if gone_for_good { "delete-permanent" } else { "recycle" }, &path,
      &format!("folder, {} files, {} MB{}", c.files, c.bytes / 1_048_576, if gone_for_good { ", deleted for good" } else { ", moved to the Recycle Bin" }));
    if let Some(t) = s.tree.lock().unwrap().as_mut() {
      t.remove(&p);
    }
    Ok(json!({ "deleted": true, "bytes": c.bytes, "permanent": gone_for_good }))
  })
  .await
}

#[tauri::command]
async fn empty_bin(s: State<'_, Shr>) -> Result<Value, String> {
  exclusive(s.inner(), |s| {
    let (bytes, items) = clean::recycle_bin();
    let r = clean::run(&["recycle".to_string()], &HashSet::new(), &jobs::Progress::new(|_| {}));
    let ok = r.first().is_some_and(|r| r.note.is_none());
    s.store.log("empty-recycle-bin", "Recycle Bin", &format!("{items} items, {} MB{}", bytes / 1_048_576, if ok { "" } else { "; Windows did not empty it" }));
    if ok { Ok(json!({ "bytes": bytes, "items": items })) } else { Err("Windows did not empty the Recycle Bin.".into()) }
  })
  .await
}

/// Hibernation on or off (`powercfg /h`). Off deletes hiberfil.sys and also turns off Hibernate and Fast Startup.
#[tauri::command]
async fn set_hibernation(s: State<'_, Shr>, on: bool) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let before = storage::hiberfil_bytes();
    let out = clean::hidden("powercfg.exe", &["/hibernate", if on { "on" } else { "off" }]).map_err(|e| e.to_string())?;
    if !out.status.success() {
      return Err(format!("Windows refused: {}", String::from_utf8_lossy(&out.stdout).trim()));
    }
    let after = storage::hiberfil_bytes();
    s.store.log(if on { "hibernation-on" } else { "hibernation-off" }, "hiberfil.sys", &format!("{} MB before, {} MB after", before / 1_048_576, after / 1_048_576));
    Ok(json!({ "on": on, "bytes": after }))
  })
  .await
  .map_err(|e| e.to_string())?
}

/// Restore points' disk use, from `vssadmin list shadowstorage` (read only). Slow, so the page asks once per visit.
#[tauri::command]
async fn get_restore_points() -> Result<Value, String> {
  tauri::async_runtime::spawn_blocking(|| {
    let out = clean::hidden("vssadmin.exe", &["list", "shadowstorage"]).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let used = text.lines().find(|l| l.contains("Used Shadow Copy Storage space")).and_then(|l| l.split(':').nth(1)).map(|v| v.trim().to_string());
    // vssadmin needs admin; without it the size is unknown rather than zero.
    Ok(json!({ "used": used, "known": out.status.success() }))
  })
  .await
  .map_err(|e| e.to_string())?
}

/// Windows' own Installed apps page; Shimmy never uninstalls anything itself.
#[tauri::command]
fn open_apps_settings() -> Result<(), String> {
  std::process::Command::new("explorer.exe").arg("ms-settings:appsfeatures").spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn storage_folder(s: State<'_, Shr>, path: Option<String>) -> Value {
  let tree = s.tree.lock().unwrap();
  let path = path.map(PathBuf::from).unwrap_or_else(|| s.profile.clone());
  tree.as_ref().and_then(|t| storage::folder(t, &path)).map(|f| json!(f)).unwrap_or(Value::Null)
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
  let p = PathBuf::from(&path);
  if p.is_dir() {
    std::process::Command::new("explorer.exe").arg(&p).spawn().map_err(|e| e.to_string())?;
  } else if p.is_file() {
    std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn().map_err(|e| e.to_string())?;
  } else {
    return Err("That file or folder no longer exists.".into());
  }
  Ok(())
}

#[tauri::command]
fn get_clean(s: State<'_, Shr>) -> Value {
  json!({ "scan": job_json(&s.clean_scan), "run": job_json(&s.clean_run), "dism": job_json(&s.dism), "finder": job_json(&s.finder) })
}

#[tauri::command]
fn start_clean_scan(s: State<'_, Shr>) -> bool {
  let s2 = s.inner().clone();
  jobs::start(&s.clean_scan, move |progress| Ok(clean::scan(&s2.running(), &progress)))
}

#[tauri::command]
fn start_clean(s: State<'_, Shr>, ids: Vec<String>) -> bool {
  run_clean(s.inner(), ids, None)
}

/// Runs the chosen clean categories as a background job. `weekly` is set when the weekly clean started it, so the
/// result is logged as such and reported in a notification.
fn run_clean(s: &Shr, ids: Vec<String>, weekly: Option<AppHandle>) -> bool {
  let s2 = s.clone();
  jobs::start(&s.clean_run, move |progress| {
    let results = clean::run(&ids, &s2.running(), &progress);
    for r in &results {
      s2.store.log(if weekly.is_some() { "weekly-clean" } else { "clean" }, r.title, &format!("{} files, {} MB freed{}", r.files, r.freed / 1_048_576,
        r.note.as_ref().map(|n| format!("; {n}")).unwrap_or_default()));
    }
    // Re-measure so the page shows what is left.
    *s2.clean_scan.lock().unwrap() = jobs::Job { result: Some(clean::scan(&s2.running(), &progress)), at: store::now_ms(), ..Default::default() };
    if let Some(app) = weekly {
      let freed: u64 = results.iter().map(|r| r.freed).sum();
      let skipped: Vec<&str> = results.iter().filter(|r| r.note.is_some() && r.freed == 0).map(|r| r.title).collect();
      let mut body = format!("Freed {} from temporary files and caches.", fmt_mb((freed / 1_048_576) as i64));
      if !skipped.is_empty() {
        body += &format!(" Skipped: {}.", skipped.join(", "));
      }
      notify(&app, "Weekly clean done", &body);
      let _ = app.emit("refresh", ());
    }
    Ok(results)
  })
}

#[tauri::command]
fn start_dism(s: State<'_, Shr>, cleanup: bool) -> bool {
  let s2 = s.inner().clone();
  jobs::start(&s.dism, move |progress| {
    let r = if cleanup { clean::dism_cleanup(&progress) } else { clean::dism_analyze(&progress) };
    if cleanup {
      s2.store.log("dism-cleanup", "Component store", &r.as_ref().map(|d| d.summary.clone()).unwrap_or_else(|e| e.clone()));
    }
    r
  })
}

#[tauri::command]
fn start_finder(s: State<'_, Shr>) -> bool {
  let (roots, rules) = (s.finder_roots(), s.finder_rules());
  jobs::start(&s.finder, move |progress| Ok(finder::scan(&roots, &rules, &progress)))
}

/// Recycles the chosen files. Each path is re-checked against the exclusions first. A file too big for the Recycle Bin
/// would be deleted for good, so it is only deleted when the page has asked a second time (allow_permanent).
#[tauri::command]
async fn finder_delete(s: State<'_, Shr>, paths: Vec<String>, allow_permanent: bool) -> Result<Value, String> {
  exclusive(s.inner(), move |s| {
    let (mut fits, mut big, mut refused) = (Vec::new(), Vec::new(), Vec::new());
    let (roots, rules) = (s.finder_roots(), s.finder_rules());
    for p in paths {
      let pb = PathBuf::from(&p);
      match finder::allowed(&pb, &roots, &rules) {
        Err(why) => refused.push(json!({ "path": p, "reason": why })),
        Ok(()) => {
          let bytes = std::fs::metadata(&pb).map(|m| m.len()).unwrap_or(0);
          if bytes > finder::bin_capacity(&pb) { big.push((pb, bytes)) } else { fits.push((pb, bytes)) }
        }
      }
    }
    if !big.is_empty() && !allow_permanent {
      return Ok(json!({ "needsConfirm": big.iter().map(|(p, b)| json!({ "path": p, "bytes": b })).collect::<Vec<_>>(), "refused": refused }));
    }
    let (mut recycled, mut recycled_bytes, mut permanent, mut permanent_bytes, mut failed, mut gone) = (0, 0u64, 0, 0u64, Vec::new(), HashSet::new());
    for (p, b) in &fits {
      match trash::delete(p) {
        Ok(()) => {
          recycled += 1;
          recycled_bytes += b;
          gone.insert(p.to_string_lossy().into_owned());
          s.store.log("recycle", &p.to_string_lossy(), &format!("{} MB, moved to the Recycle Bin", b / 1_048_576));
        }
        Err(e) => failed.push(json!({ "path": p, "reason": e.to_string() })),
      }
    }
    for (p, b) in &big {
      match std::fs::remove_file(p) {
        Ok(()) => {
          permanent += 1;
          permanent_bytes += b;
          gone.insert(p.to_string_lossy().into_owned());
          s.store.log("delete-permanent", &p.to_string_lossy(), &format!("{} MB, too big for the Recycle Bin, deleted after confirmation", b / 1_048_576));
        }
        Err(e) => failed.push(json!({ "path": p, "reason": e.to_string() })),
      }
    }
    // Drop what was removed from the cached results, so the list updates without a rescan.
    if let Some(found) = s.finder.lock().unwrap().result.as_mut() {
      found.large.retain(|i| !gone.contains(&i.path));
      found.installers.retain(|i| !gone.contains(&i.path));
      for g in found.dups.iter_mut() {
        g.files.retain(|i| !gone.contains(&i.path));
      }
      found.dups.retain(|g| g.files.len() > 1);
    }
    Ok(json!({ "recycled": recycled, "recycledBytes": recycled_bytes, "permanent": permanent, "permanentBytes": permanent_bytes, "failed": failed, "refused": refused }))
  })
  .await
}

// ---------- Battery ----------

#[tauri::command]
async fn get_battery(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let now = store::now_ms();
    let fresh = s.battery_report.lock().unwrap().as_ref().is_some_and(|(at, _)| now.saturating_sub(*at) < 600_000)
      || now.saturating_sub(*s.battery_report_failed.lock().unwrap()) < 600_000;
    let mut error = None;
    if !fresh {
      match battery::report() {
        Ok(r) => *s.battery_report.lock().unwrap() = Some((now, r)),
        Err(e) => {
          *s.battery_report_failed.lock().unwrap() = now;
          error = Some(e);
        }
      }
    }
    let live = system::home(&mut s.sys.lock().unwrap()).battery;
    let rows = s.readings().0;
    let report = s.battery_report.lock().unwrap().as_ref().map(|(_, r)| r.clone());
    let limit = s.settings.lock().unwrap().charge_limit;
    let view = battery::build(&live, rows.as_deref(), report.as_ref(), limit, s.limit_supported());
    let mode = s.settings.lock().unwrap().plugged.clone();
    let hold = json!({
      "on": mode.on,
      "holdPct": mode.hold_pct,
      "text": s.battery_reading().map_or_else(|| "Shimmy cannot read the battery's charge right now.".into(), |r| hold::describe(&mode, &r)),
      "myasus": hold::myasus_limit(),
      "overridden": s.hold_watch.lock().unwrap().override_warned,
    });
    let (gpu_present, gpu_awake, gpu_w) = s.rtx_power();
    let cpu_w = rows.as_deref().and_then(|r| sensors::value(r, "Cpu", "Power", "Package"));
    let total = battery::total_draw(view.plugged, view.rate_w, cpu_w, gpu_w.unwrap_or(0.0), *s.rest_w.lock().unwrap());
    let power = json!({
      "batteryW": view.rate_w,
      "cpuW": cpu_w,
      "gpuPresent": gpu_present,
      "gpuAwake": gpu_awake,
      "gpuW": gpu_w,
      "totalW": total.map(|t| t.0),
      "totalExact": total.is_some_and(|t| t.1),
      "draining": view.plugged && view.rate_w.is_some_and(|w| w < -0.5),
    });
    let hist: Vec<Value> = s.power_hist.lock().unwrap().iter().map(|p| json!([p.t, p.battery_w, p.cpu_w, p.plugged, p.held])).collect();
    json!({ "battery": view, "hold": hold, "power": power, "powerHistory": hist, "error": error })
  })
  .await
  .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_charge_limit(s: State<'_, Shr>, pct: u32) -> Result<Value, String> {
  if !(50..=100).contains(&pct) {
    return Err("The charge limit must be between 50% and 100%.".into());
  }
  if s.settings.lock().unwrap().plugged.on {
    return Err("Turn off Run from the charger first; it sets the limit itself while it is on.".into());
  }
  let result = s.write_limit(pct)?;
  s.settings.lock().unwrap().charge_limit = Some(pct);
  s.save_settings();
  s.store.log("charge-limit", &format!("{pct}%"), &format!("ASUS firmware answered {result}"));
  Ok(json!({ "ok": true, "result": result }))
}

/// Turns Run from the charger on (hold at the current charge, never below 40 %) or off (back to the 60/80/100 choice).
/// Returns the limit written.
fn plugged_mode_impl(s: &Shared, on: bool) -> Result<u32, String> {
  if !s.limit_supported() {
    return Err("This laptop's firmware does not offer a charge limit, which Run from the charger needs.".into());
  }
  if on {
    if !system::battery_now().present {
      return Err("No battery found.".into());
    }
    let charge = s.battery_reading().ok_or("Shimmy cannot read the battery's charge right now.")?.charge_pct;
    let h = hold::hold_for(charge);
    s.write_limit(h)?;
    s.settings.lock().unwrap().plugged = hold::PluggedMode { on: true, hold_pct: h };
    s.save_settings();
    *s.hold_watch.lock().unwrap() = hold::Watch::default();
    s.store.log("plugged-mode-on", &format!("{h}%"), &format!("battery at {charge:.0}%; it stops charging there and the charger powers the laptop"));
    Ok(h)
  } else {
    let back = s.settings.lock().unwrap().charge_limit.unwrap_or(100);
    s.write_limit(back)?;
    s.settings.lock().unwrap().plugged.on = false;
    s.save_settings();
    s.store.log("plugged-mode-off", &format!("{back}%"), "charge limit back to the previous choice");
    Ok(back)
  }
}

fn sync_tray_check(app: &AppHandle, s: &Shared) {
  let on = s.settings.lock().unwrap().plugged.on;
  if let Some(t) = app.try_state::<TrayPlugged>() {
    let _ = t.0.set_checked(on);
  }
}

#[tauri::command]
async fn set_plugged_mode(app: AppHandle, s: State<'_, Shr>, on: bool) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let r = plugged_mode_impl(&s, on);
    sync_tray_check(&app, &s);
    let limit = r?;
    Ok(json!({ "on": on, "limit": limit }))
  })
  .await
  .map_err(|e| e.to_string())?
}

/// Run from the charger, every 3 s: hold again at each plug-in, rewrite the limit if the battery charges past it, and
/// say so when the battery has to help the charger.
fn plugged_watch(app: &AppHandle, s: &Shared, now: u64) {
  let mode = s.settings.lock().unwrap().plugged.clone();
  if s.atk.is_none() {
    return;
  }
  let Some(r) = s.battery_reading() else { return };
  let actions = s.hold_watch.lock().unwrap().step(&mode, &r, now);
  for a in actions {
    match a {
      hold::Action::Hold(h) => {
        if s.write_limit(h).is_ok() {
          s.settings.lock().unwrap().plugged.hold_pct = h;
          s.save_settings();
          s.store.log("charge-hold", &format!("{h}%"), &format!("plugged in at {:.0}%, holding there", r.charge_pct));
          let _ = app.emit("refresh", ());
        } else {
          s.hold_watch.lock().unwrap().retry_plug_in();
        }
      }
      hold::Action::Reapply => {
        let _ = s.write_limit(mode.hold_pct);
        s.store.log("charge-hold", &format!("{}%", mode.hold_pct), &format!("battery was charging above the hold at {:.0}%; limit written again", r.charge_pct));
      }
      hold::Action::WarnOverride => alert(app, s, "The battery is charging past the hold",
        &format!("Shimmy set the limit to {}% twice, but the battery keeps charging. MyASUS Battery Care may be changing it: set it to the same value there, or turn it off.", mode.hold_pct)),
      hold::Action::WarnDrain(w) => alert(app, s, "The battery is helping the charger",
        &format!("{w:.0} W are coming from the battery while plugged in, because the charger cannot keep up with this load. Shimmy still holds it at {}% once the load drops.", mode.hold_pct)),
    }
  }
}

// ---------- Fan mode and heat protection ----------

#[tauri::command]
async fn set_fan_mode(s: State<'_, Shr>, mode: String) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let atk = s.atk.as_ref().ok_or("The ASUS control interface is not available.")?;
    atk.fan_mode().ok_or("This laptop's firmware does not offer fan modes.")?;
    atk.set_fan_mode(&mode)?;
    s.store.log("fan-mode", &mode, "set from the Processor page");
    Ok(json!({ "mode": atk.fan_mode() }))
  })
  .await
  .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_heat_protection(s: State<'_, Shr>, on: bool) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    s.settings.lock().unwrap().heat.enabled = on;
    s.save_settings();
    if !on {
      s.restore_boost("heat protection turned off");
    }
    *s.heat_watch.lock().unwrap() = heat::Watch::default();
    Ok(json!({ "on": on }))
  })
  .await
  .map_err(|e| e.to_string())?
}

/// Heat protection, every 3 s: boost off after 30 s at 90 °C or more, back on after 2 min at 75 °C or less. Quiet: the
/// Log and the Processor page only.
fn heat_watch(s: &Shared, now: u64) {
  let (enabled, boost_off) = {
    let st = s.settings.lock().unwrap();
    (st.heat.enabled, st.heat.saved.is_some())
  };
  let temp = s.readings().0.and_then(|r| sensors::value(&r, "Cpu", "Temperature", "Core (Tctl/Tdie)"));
  let action = s.heat_watch.lock().unwrap().step(enabled, boost_off, temp, now);
  match action {
    Some(heat::Action::BoostOff) => match heat::boost_off(now) {
      Ok(saved) => {
        let what = format!("AC {}, DC {} before", saved.ac, saved.dc);
        s.settings.lock().unwrap().heat.saved = Some(saved);
        s.save_settings();
        s.heat_failed.store(false, Ordering::SeqCst);
        s.store.log("boost-off", &what, &format!("processor at {:.0} °C for 30 s; boost off until it cools to {:.0} °C", temp.unwrap_or(0.0), heat::COOL_C));
      }
      Err(e) => {
        if !s.heat_failed.swap(true, Ordering::SeqCst) {
          s.store.log("boost-off-failed", "heat protection", &e);
        }
      }
    },
    Some(heat::Action::BoostOn) => {
      let why = match temp {
        _ if !enabled => "heat protection turned off".to_string(),
        Some(t) => format!("cooled to {t:.0} °C for 2 min"),
        None => "no temperature reading for 2 min".to_string(),
      };
      s.restore_boost(&why);
    }
    None => {}
  }
}

// ---------- Settings ----------

fn settings_json(s: &Shared) -> Value {
  let st = s.settings.lock().unwrap().clone();
  let next = st.weekly.enabled.then(|| auto::next_slot(store::now_ms(), &auto::local_now(), st.weekly.day, st.weekly.hour));
  json!({
    "settings": st,
    "startWithWindows": autostart::enabled(),
    "defaultRoots": s.default_roots,
    "categories": clean::categories().into_iter().map(|(id, title)| json!({ "id": id, "title": title })).collect::<Vec<_>>(),
    "nextWeekly": next,
  })
}

#[tauri::command]
async fn get_settings(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || settings_json(&s)).await.map_err(|e| e.to_string())
}

/// A folder the finder may search: never Windows, program or app-data folders, nor the whole of C:.
fn check_root(p: &Path) -> Result<(), String> {
  if !p.is_absolute() || !p.is_dir() {
    return Err(format!("{} is not a folder on this PC.", p.display()));
  }
  let low = p.to_string_lossy().to_lowercase();
  let system = ["\\windows", "\\program files", "\\programdata", "\\appdata", "$recycle.bin", "system volume information"];
  if (p.parent().is_none() && low.starts_with("c:")) || system.iter().any(|b| low.contains(b)) {
    return Err(format!("{} holds Windows or app files, so the finder will not search it.", p.display()));
  }
  Ok(())
}

#[tauri::command]
async fn set_settings(s: State<'_, Shr>, settings: Value) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let mut new: Settings = serde_json::from_value(settings).map_err(|e| format!("Settings not understood: {e}"))?;
    auto::clamp(&mut new.alerts, &mut new.auto_clean, &mut new.weekly, &mut new.finder);
    new.caches.age_days = new.caches.age_days.clamp(1, 365);
    for r in &new.finder.extra_roots {
      check_root(Path::new(r))?;
    }
    {
      let mut cur = s.settings.lock().unwrap();
      // Kept by Shimmy, not by the page.
      new.charge_limit = cur.charge_limit;
      new.plugged = cur.plugged.clone();
      new.heat.saved = cur.heat.saved.clone();
      new.weekly.last_run = cur.weekly.last_run;
      // Turning the weekly clean on, or moving it, counts from now, so a slot already in the past does not fire at once.
      let w = &new.weekly;
      if w.enabled && (!cur.weekly.enabled || w.day != cur.weekly.day || w.hour != cur.weekly.hour) {
        new.weekly.last_run = store::now_ms();
      }
      *cur = new;
    }
    s.save_settings();
    Ok(settings_json(&s))
  })
  .await
  .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_autostart(app: AppHandle, s: State<'_, Shr>, on: bool) -> Result<Value, String> {
  if app.config().identifier.ends_with(".dev") {
    return Err("Start with Windows can only be set from the real Shimmy, not a development build.".into());
  }
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    autostart::set(on)?;
    s.store.log(if on { "autostart-on" } else { "autostart-off" }, autostart::TASK, "Task Scheduler task at sign-in, highest privileges, starts in the tray");
    Ok(json!({ "on": autostart::enabled() }))
  })
  .await
  .map_err(|e| e.to_string())?
}

// ---------- Window, tray, background ----------

/// Opens the window, building it again if it was closed. Closing destroys the window and its WebView2 engine (about
/// 180 MB) instead of hiding it; everything else runs in this process without it.
fn show_main(app: &AppHandle) {
  if let Some(w) = app.get_webview_window("main") {
    let _ = w.unminimize();
    let _ = w.show();
    let _ = w.set_focus();
    return;
  }
  static BUILDING: AtomicBool = AtomicBool::new(false);
  if BUILDING.swap(true, Ordering::SeqCst) {
    return;
  }
  // Building a webview inside an event handler can deadlock on Windows, so it is built from another thread.
  let app = app.clone();
  std::thread::spawn(move || {
    if let Some(cfg) = app.config().app.windows.iter().find(|w| w.label == "main").cloned() {
      if let Ok(w) = tauri::WebviewWindowBuilder::from_config(&app, &cfg).and_then(|b| b.build()) {
        let _ = w.show();
        let _ = w.set_focus();
      }
    }
    BUILDING.store(false, Ordering::SeqCst);
  });
}

fn notify(app: &AppHandle, title: &str, body: &str) {
  let _ = app.notification().builder().title(title).body(body).show();
}

/// Clean now or Trim from the tray, or from the automatic memory clean (`auto`), with a notification of the result.
fn tray_clean(app: AppHandle, trim_only: bool, auto: bool) {
  let s: Shr = app.state::<Shr>().inner().clone();
  std::thread::spawn(move || {
    if s.busy.swap(true, Ordering::SeqCst) {
      return;
    }
    let r = if trim_only { trim_impl(&s) } else { quick_clean_impl(&s) };
    s.busy.store(false, Ordering::SeqCst);
    let freed = r["freedMB"].as_i64().unwrap_or(0);
    let title = match (auto, freed > 0) {
      (true, true) => format!("Memory was full, Shimmy freed {}", fmt_mb(freed)),
      (true, false) => "Memory was full, Shimmy cleaned up".to_string(),
      (false, true) => format!("Freed {}", fmt_mb(freed)),
      (false, false) => "Done".to_string(),
    };
    let trimmed = r["trimmed"].as_u64().unwrap_or(0);
    let list = |k: &str| r[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let mut body = if trim_only {
      format!("Trimmed {trimmed} apps. Nothing was closed.")
    } else if list("closedApps").is_empty() {
      format!("Nothing idle to close. Trimmed {trimmed} apps.")
    } else {
      format!("Closed {}. Trimmed {trimmed} apps.", list("closedApps"))
    };
    if !list("skippedApps").is_empty() {
      body += &format!(" Left open (has a window): {}.", list("skippedApps"));
    }
    notify(&app, &title, &body);
    let _ = app.emit("refresh", ());
  });
}

fn update_tray(app: &AppHandle, last: &AtomicU64) {
  let m = system::mem_now();
  let text = format!("RAM {}% · {} free of {}", m.pct, fmt_mb(m.free_mb as i64), fmt_mb(m.total_mb as i64));
  if let Some(t) = app.tray_by_id("main") {
    let _ = t.set_tooltip(Some(format!("Shimmy Optimizer · {text}")));
    if last.swap(m.pct, Ordering::SeqCst) != m.pct {
      let _ = t.set_icon(Some(Image::new_owned(tray::ring_rgba(m.pct), tray::SIZE, tray::SIZE)));
    }
  }
  let _ = app.state::<TrayStatus>().0.set_text(text);
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
  let status = MenuItem::with_id(app, "status", "RAM", false, None::<&str>)?;
  let shared = app.state::<Shr>();
  let plugged_on = shared.settings.lock().unwrap().plugged.on;
  let plugged = CheckMenuItem::with_id(app, "plugged", "Run from the charger", shared.limit_supported(), plugged_on, None::<&str>)?;
  let menu = Menu::with_items(app, &[
    &status,
    &PredefinedMenuItem::separator(app)?,
    &MenuItem::with_id(app, "clean", "Clean now", true, None::<&str>)?,
    &MenuItem::with_id(app, "trim", "Trim memory only", true, None::<&str>)?,
    &plugged,
    &MenuItem::with_id(app, "open", "Open Shimmy", true, None::<&str>)?,
    &PredefinedMenuItem::separator(app)?,
    &MenuItem::with_id(app, "exit", "Exit", true, None::<&str>)?,
  ])?;
  app.manage(TrayStatus(status));
  app.manage(TrayPlugged(plugged));
  TrayIconBuilder::with_id("main")
    .icon(Image::new_owned(tray::ring_rgba(system::mem_now().pct), tray::SIZE, tray::SIZE))
    .tooltip("Shimmy Optimizer")
    .menu(&menu)
    .show_menu_on_left_click(false)
    .on_menu_event(|app, e| match e.id().as_ref() {
      "clean" => tray_clean(app.clone(), false, false),
      "trim" => tray_clean(app.clone(), true, false),
      "plugged" => {
        let (app, s) = (app.clone(), app.state::<Shr>().inner().clone());
        std::thread::spawn(move || {
          let on = !s.settings.lock().unwrap().plugged.on;
          match plugged_mode_impl(&s, on) {
            Ok(limit) if on => notify(&app, "Running from the charger", &format!("The battery stays at {limit}% while plugged in; the charger powers the laptop.")),
            Ok(limit) => notify(&app, "Charging normally again", &format!("Charge limit back to {limit}%.")),
            Err(e) => notify(&app, "Run from the charger", &e),
          }
          sync_tray_check(&app, &s);
          let _ = app.emit("refresh", ());
        });
      }
      "open" => show_main(app),
      "exit" => {
        let s = app.state::<Shr>();
        s.restore_boost("Shimmy exited");
        s.store.save();
        app.exit(0);
      }
      _ => {}
    })
    .on_tray_icon_event(|tray, e| {
      if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
        show_main(tray.app_handle());
      }
    })
    .build(app)?;
  Ok(())
}

fn alert(app: &AppHandle, s: &Shared, title: &str, body: &str) {
  notify(app, title, body);
  s.store.log("alert", title, body);
}

/// Alerts, the automatic memory clean and the weekly clean. Called by the sampler every 3 s.
fn automation(app: &AppHandle, s: &Shr, tick: u64) {
  const MIN: u64 = 60_000;
  let now = store::now_ms();
  let st = s.settings.lock().unwrap().clone();
  let m = system::mem_now();
  plugged_watch(app, s, now);
  heat_watch(s, now);
  let mut w = s.watch.lock().unwrap();
  if st.alerts.enabled {
    let a = &st.alerts;
    if w.ram.step(m.pct >= a.ram_pct as u64, now, MIN, 30 * MIN) {
      alert(app, s, "Memory is nearly full", &format!("{}% in use for over a minute. Open Shimmy to see what to close.", m.pct));
    }
    let temp = s.readings().0.and_then(|r| sensors::value(&r, "Cpu", "Temperature", "Core (Tctl/Tdie)"));
    if w.cpu.step(temp.is_some_and(|t| t >= a.cpu_temp_c as f64), now, 30_000, 30 * MIN) {
      alert(app, s, "The processor is running hot", &format!("{:.0} °C for half a minute. Check the vents are clear and nothing is stuck at full load.", temp.unwrap_or(0.0)));
    }
    if tick % 100 == 0 {
      let free_gb = storage::space("C:\\").1 / 1_073_741_824;
      if w.disk.step(free_gb < a.disk_free_gb as u64, now, 0, 6 * 60 * MIN) {
        alert(app, s, "C: is nearly full", &format!("Only {free_gb} GB free. The Clean page can make room."));
      }
    }
    let b = system::battery_now();
    let low = b.present && !b.plugged && b.pct.is_some_and(|p| (p as u32) <= a.battery_pct);
    if w.battery.step(low, now, 0, 60 * MIN) {
      alert(app, s, "Battery is low", &format!("{}% left. Plug in soon.", b.pct.unwrap_or(0)));
    }
  }
  let c = &st.auto_clean;
  if c.enabled && w.auto_clean.step(m.pct >= c.at_pct as u64, now, c.for_min as u64 * MIN, 15 * MIN) {
    s.store.log("auto-clean", if c.level == "clean" { "Clean now" } else { "Trim" }, &format!("memory at {}% for {} min or more", m.pct, c.for_min));
    tray_clean(app.clone(), c.level != "clean", true);
  }
  drop(w);
  if tick % 20 == 10 && !st.weekly.cats.is_empty() && auto::weekly_due(&st.weekly, now, &auto::local_now()) && !s.clean_run.lock().unwrap().running {
    s.settings.lock().unwrap().weekly.last_run = now;
    s.save_settings();
    run_clean(s, st.weekly.cats.clone(), Some(app.clone()));
  }
}

/// Every 3 s: tray icon and status, automation. Every 10 s: a RAM history sample. Every minute: save history.
fn start_sampler(app: AppHandle) {
  std::thread::spawn(move || {
    let last = AtomicU64::new(u64::MAX);
    for tick in 0u64.. {
      update_tray(&app, &last);
      let s = app.state::<Shr>();
      automation(&app, s.inner(), tick);
      s.sample_power();
      if tick % 600 == 0 {
        s.reapply_charge_limit();
      }
      if tick % 3 == 0 {
        s.store.sample(system::mem_now().used_mb);
        let load = {
          let mut sys = s.sys.lock().unwrap();
          sys.refresh_cpu_usage();
          sys.global_cpu_usage()
        };
        let temp = s.readings().0.and_then(|r| sensors::value(&r, "Cpu", "Temperature", "Core (Tctl/Tdie)"));
        s.store.sample_cpu(load, temp.map(|t| t as f32));
      }
      if tick % 20 == 19 {
        s.store.save();
      }
      std::thread::sleep(Duration::from_secs(3));
    }
  });
}

/// `--autostart on|off`, run by the installer hooks (installer-hooks.nsh). Returns the exit code, or None to start normally.
pub fn installer_step() -> Option<i32> {
  let args: Vec<String> = std::env::args().collect();
  let i = args.iter().position(|a| a == "--autostart")?;
  let on = args.get(i + 1).is_none_or(|v| v != "off");
  Some(if autostart::set(on).is_ok() { 0 } else { 1 })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
    .plugin(tauri_plugin_notification::init())
    .setup(|app| {
      let dir = app.path().app_data_dir()?;
      let settings_path = dir.join("settings.json");
      let settings: Settings = std::fs::read_to_string(&settings_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
      let store = Store::open(dir);
      if let Ok(desktop) = app.path().desktop_dir() {
        if let Some(what) = store.import_old(&desktop.join("RAM Optimizer").join("data")) {
          store.log("import", "RAM Optimizer v0.2", &format!("Imported {what}"));
        }
      }
      store.sample(system::mem_now().used_mb);
      let mut sys = System::new();
      sys.refresh_cpu_all();
      let sensor_state = sensors::SensorState::default();
      sensors::start(app.path().resource_dir()?.join("sensors").join("shimmy-sensors.exe"), sensor_state.clone());
      app.manage::<Shr>(Arc::new(Shared {
        sys: Mutex::new(sys),
        store,
        busy: AtomicBool::new(false),
        sensors: sensor_state,
        atk: asus::Atk::open(),
        nvml: Mutex::new(None),
        base_mhz: cpu::base_mhz(),
        profile: app.path().home_dir()?,
        default_roots: [app.path().download_dir(), app.path().desktop_dir(), app.path().document_dir(), app.path().video_dir(), app.path().picture_dir()]
          .into_iter()
          .flatten()
          .collect(),
        usage: Default::default(),
        tree: Mutex::new(None),
        clean_scan: Default::default(),
        clean_run: Default::default(),
        dism: Default::default(),
        finder: Default::default(),
        cache_scan: Default::default(),
        cache_run: Default::default(),
        gpu_counters: Mutex::new(None),
        watch: Mutex::new(auto::Watch::default()),
        hold_watch: Mutex::new(hold::Watch::default()),
        heat_watch: Mutex::new(heat::Watch::default()),
        heat_failed: AtomicBool::new(false),
        power_hist: Mutex::new(VecDeque::with_capacity(battery::HIST_POINTS)),
        rest_w: Mutex::new(None),
        battery_report: Mutex::new(None),
        battery_report_failed: Mutex::new(0),
        settings: Mutex::new(settings),
        settings_path,
      }));
      // Shimmy was closed (or the PC restarted) while it had boost off: put it back before anything else.
      app.state::<Shr>().restore_boost("Shimmy was closed while boost was off");
      build_tray(app.handle())?;
      start_sampler(app.handle().clone());
      // The sign-in task starts Shimmy in the tray; any other start opens the window.
      if !std::env::args().any(|a| a == autostart::TRAY_ARG) {
        show_main(app.handle());
      }
      Ok(())
    })
    .on_window_event(|w, e| {
      // Closing the window destroys it; Shimmy stays in the tray (Exit is in the tray menu). What only the window
      // needed is freed with it.
      if let WindowEvent::Destroyed = e {
        let s = w.app_handle().state::<Shr>().inner().clone();
        std::thread::spawn(move || s.release());
      }
    })
    .invoke_handler(tauri::generate_handler![get_state, get_history, get_gpu, get_storage, start_usage_scan, storage_folder, open_path, get_clean, start_clean_scan, start_clean, start_dism, start_finder, finder_delete, get_battery, set_charge_limit, set_fan_mode, set_heat_protection, get_settings, set_settings, set_autostart, set_plugged_mode, start_cache_scan, start_cache_clean, folder_preview, folder_delete, empty_bin, set_hibernation, get_restore_points, open_apps_settings, get_log, get_startup, toggle_startup, close_procs, trim, quick_clean])
    .build(tauri::generate_context!())
    .expect("error while running Shimmy Optimizer")
    .run(|_app, e| {
      // With no window open Shimmy keeps running in the tray; only the tray's Exit (an explicit exit code) quits.
      if let tauri::RunEvent::ExitRequested { api, code: None, .. } = e {
        api.prevent_exit();
      }
    });
}
