pub mod asus;
pub mod auto;
pub mod autostart;
pub mod battery;
pub mod caches;
pub mod clean;
pub mod cool;
pub mod cpu;
pub mod finder;
pub mod gpu;
pub mod heat;
pub mod hold;
pub mod hw;
pub mod jobs;
pub mod net;
pub mod power;
pub mod prio;
pub mod procs;
pub mod programs;
pub mod report;
pub mod senselog;
pub mod sensors;
pub mod storage;
pub mod startup;
pub mod store;
pub mod system;
pub mod tray;
pub mod update;

use procs::{build_groups, classify, recommend, Marks};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
  /// Which logical processors share a core, read once at start.
  topology: cpu::Topology,
  /// Busy % and effective clocks, written only by the sampler (every 3 s) so the window is always the same length.
  cpu_live: Mutex<cpu::Live>,
  /// The laptop's model from the BIOS, and whether the ASUS firmware writes were tested on it (asus.rs).
  model: String,
  model_tested: bool,
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
  /// The last check's answer, for the Updates page and the download (the page never supplies a URL).
  update_info: Mutex<Option<update::Release>>,
  update_dl: jobs::Slot<update::Downloaded>,
  /// Bytes of the installer downloaded so far and in all.
  update_done: AtomicU64,
  update_total: AtomicU64,
  /// One check at a time, whether it came from the button or the daily timer.
  update_busy: AtomicBool,
  /// The charger state power mode last acted on (power.rs), so it switches only when that changes.
  power_last: Mutex<Option<bool>>,
  /// The cooling check (cool.rs) and its Stop button.
  cool: jobs::Slot<cool::Verdict>,
  cool_stop: Arc<AtomicBool>,
  /// The System page's hardware list (hw.rs), read once.
  hw_info: Mutex<Option<Value>>,
  /// Programs page (programs.rs): winget's list of updates (None inside when winget is missing), the one update
  /// running, and the one uninstaller Shimmy started.
  app_updates: jobs::Slot<Option<Vec<programs::Upgrade>>>,
  app_update_run: jobs::Slot<Value>,
  uninstaller: Mutex<Option<std::process::Child>>,
  /// Processes Shimmy lowered (prio.rs), so each can be put back.
  prio_applied: Mutex<prio::Applied>,
  /// The sensor log (senselog.rs) and its Stop button.
  sensor_log: jobs::Slot<senselog::Done>,
  sensor_log_stop: Arc<AtomicBool>,
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
  /// The daily check for a new version (update.rs).
  updates: auto::UpdatePrefs,
  /// Power mode follows the charger (power.rs).
  power: power::PowerPrefs,
  /// The sensor log's last choice (senselog.rs). Changed only by `start_sensor_log`; kept by Shimmy.
  sensor_log: senselog::Prefs,
  /// When Shimmy last made a restore point (ms). Kept by Shimmy.
  restore_point_at: u64,
  /// The user accepted the warning that ASUS firmware writes were not tested on this model. Kept by Shimmy.
  asus_untested_ok: bool,
  /// Per-app marks by lower-case exe name (procs.rs). Changed only by `set_app_mark`; kept by Shimmy.
  #[serde(deserialize_with = "procs::lenient_marks")]
  app_marks: Marks,
  /// Background priority by lower-case exe name (prio.rs). Changed only by `set_priority`; kept by Shimmy.
  #[serde(deserialize_with = "prio::lenient")]
  priorities: BTreeMap<String, prio::Level>,
}

/// Starts the error a firmware write returns on an untested model before the user's OK; the page shows the warning.
const UNTESTED: &str = "UNTESTED_MODEL:";

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

  /// ASUS firmware writes on a model they were not tested on need the user's OK once. `confirmed` is that OK.
  fn check_model(&self, confirmed: bool) -> Result<(), String> {
    if self.model_tested || self.settings.lock().unwrap().asus_untested_ok {
      return Ok(());
    }
    if !confirmed {
      return Err(format!("{UNTESTED}{}", self.model));
    }
    self.settings.lock().unwrap().asus_untested_ok = true;
    self.save_settings();
    let model = if self.model.is_empty() { "unknown model" } else { &self.model };
    self.store.log("asus-untested-ok", model, &format!("accepted ASUS firmware controls tested only on the {}", asus::TESTED_MODEL));
    Ok(())
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

  /// The dedicated graphics card for the Battery page: its short name (None when there is none), whether it is awake
  /// (not `asleep` in gpu.rs) and, only then and only for NVIDIA, its watts from NVML. The graphics kernel gives no
  /// watts, so other vendors' cards have none. Asking NVML wakes a sleeping card, so it is never asked while it sleeps.
  fn dgpu_power(&self) -> (Option<String>, bool, Option<f64>) {
    let card = match self.gpu_counters.lock().unwrap().get_or_insert_with(gpu::Counters::open) {
      Ok(c) => {
        let adapters = c.sample(&HashMap::new()).0;
        let mut dedicated: Vec<_> = adapters.into_iter().filter(|a| !a.integrated).collect();
        // The busiest one, if there are several.
        dedicated.sort_by(|a, b| b.load_pct.total_cmp(&a.load_pct));
        dedicated.into_iter().next()
      }
      Err(_) => None,
    };
    let Some(card) = card else { return (None, false, None) };
    // Asleep only when idle next to a built-in chip; a card that runs the screen itself is always awake.
    let awake = !card.asleep;
    if !awake || card.vendor != "NVIDIA" {
      return (Some(card.short), awake, None);
    }
    let w = match self.nvml.lock().unwrap().get_or_insert_with(|| nvml_wrapper::Nvml::init().map_err(|e| e.to_string())) {
      Ok(n) => gpu::nvidia(n).ok().and_then(|g| g.power_w),
      Err(_) => None,
    };
    (Some(card.short), true, w)
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

  /// Puts back the power mode Shimmy changed. Used when "follows the charger" is turned off, at Exit, and at start-up
  /// when Shimmy was closed without putting it back.
  fn restore_power(&self, why: &str) {
    let Some(saved) = self.settings.lock().unwrap().power.saved.clone() else { return };
    *self.power_last.lock().unwrap() = None;
    let clear = || {
      self.settings.lock().unwrap().power.saved = None;
      self.save_settings();
    };
    let Some(g) = power::from_text(&saved) else { return clear() };
    match power::set(g) {
      Ok(()) => {
        clear();
        self.store.log("power-mode", power::label(g), &format!("put back: {why}"));
      }
      Err(e) => self.store.log("power-mode-failed", power::label(g), &e),
    }
  }

  /// A restore point before a change the Recycle Bin cannot undo, at most one a day (Windows' own limit). Logged either
  /// way; returns a line for the page.
  fn restore_point_first(&self, before: &str) -> String {
    let now = store::now_ms();
    if now.saturating_sub(self.settings.lock().unwrap().restore_point_at) < 86_400_000 {
      self.store.log("restore-point-skipped", before, "Shimmy made one in the last 24 hours; Windows makes at most one a day");
      return "Shimmy made a restore point in the last 24 hours, so it did not make another.".into();
    }
    match clean::restore_point(before) {
      Ok(()) => {
        self.settings.lock().unwrap().restore_point_at = now;
        self.save_settings();
        self.store.log("restore-point", before, "made by System Restore before the change");
        "Windows made a restore point first.".into()
      }
      Err(e) => {
        self.store.log("restore-point-failed", before, &e);
        format!("No restore point: {e}")
      }
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
    jobs::clear(&self.cool);
    *self.hw_info.lock().unwrap() = None;
    *self.battery_report.lock().unwrap() = None;
    *self.nvml.lock().unwrap() = None;
    *self.gpu_counters.lock().unwrap() = None;
    {
      // Only the processor list is wanted back (list_procs divides by it); nothing reads this System's usage. On Windows
      // there is no list-only refresh: the list is built by the first refresh, and one call is cheap.
      let mut sys = self.sys.lock().unwrap();
      *sys = System::new();
      sys.refresh_cpu_all();
    }
    system::compact_self();
  }

  /// The user's per-app marks.
  fn marks(&self) -> Marks {
    self.settings.lock().unwrap().app_marks.clone()
  }

  /// Lower-case names of running processes, fresh.
  fn running(&self) -> HashSet<String> {
    let mut sys = self.sys.lock().unwrap();
    system::list_procs(&mut sys).into_iter().map(|p| p.name.to_lowercase()).collect()
  }

  fn fans(&self) -> Vec<cpu::Fan> {
    self.atk.as_ref().map_or(Vec::new(), fans_of)
  }

  /// Everything the sensor log can write: Shimmy's own readings, which work without administrator rights, then the
  /// sensor reader's.
  fn log_rows(&self) -> Vec<sensors::Reading> {
    let rows = self.readings().0.unwrap_or_default();
    let live = self.cpu_live.lock().unwrap().clone();
    let view = cpu::build(String::new(), &self.topology, &live, self.base_mhz, Some(&rows), Vec::new(), None);
    let own = |sensor: &str, kind: &str, value: Option<f64>| sensors::Reading { hw: senselog::OWN_HW.into(), hw_type: "Shimmy".into(), sensor: sensor.into(), kind: kind.into(), value };
    let battery = system::battery_now();
    let mut out = vec![own("Processor use", "Load", Some(live.total as f64)), own("Processor clock", "Clock", view.avg_clock_mhz), own("Memory in use", "Load", Some(system::mem_now().pct as f64))];
    if battery.present {
      out.push(own("Battery charge", "Level", battery.pct.map(f64::from)));
    }
    out.extend(self.fans().into_iter().map(|f| own(&f.name, "Fan", Some(f.rpm as f64))));
    out.extend(rows);
    out
  }
}

fn fans_of(atk: &asus::Atk) -> Vec<cpu::Fan> {
  [("Processor fan", asus::CPU_FAN), ("Graphics fan", asus::GPU_FAN)]
    .into_iter()
    .filter_map(|(name, id)| atk.fan_rpm(id).map(|rpm| cpu::Fan { name: name.into(), rpm }))
    .collect()
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

/// Trims every app's memory except protected ones: built-in protected processes and apps marked Protected.
fn trim_impl(s: &Shared) -> Value {
  let marks = s.marks();
  let before = free_bytes();
  let (n, left) = {
    let mut sys = s.sys.lock().unwrap();
    let procs_now = system::list_procs(&mut sys);
    let skip = procs::trim_skip(&procs_now, &system::own_pids(&procs_now), &marks);
    (system::trim_all(&sys, &skip), skip.len())
  };
  std::thread::sleep(Duration::from_millis(1000));
  let freed = mb_of(free_bytes() - before);
  s.store.log("trim-working-sets", "all but protected", &format!("{n} processes trimmed, {left} protected left alone, {freed} MB change in available memory"));
  json!({ "trimmed": n, "freedMB": freed })
}

/// Closes only the apps marked Safe (never a protected one), then trims the rest. An app with a window is asked to
/// close and may stay open; one without a window is ended. Shared by the buttons, the tray and the automatic clean.
fn quick_clean_impl(s: &Shared) -> Value {
  let marks = s.marks();
  let procs_now = system::list_procs(&mut s.sys.lock().unwrap());
  let own = system::own_pids(&procs_now);
  let plan = procs::clean_plan(&procs_now, &own, &marks);
  let before = free_bytes();
  let (mut targets, mut closed, mut left) = (Vec::new(), 0u64, HashSet::new());
  for (list, force) in [(plan.force, true), (plan.ask, false)] {
    if list.is_empty() {
      continue;
    }
    targets.extend(list.iter().cloned());
    let r = close_impl(s, list, force);
    closed += r["closed"].as_u64().unwrap_or(0);
    left.extend(r["remaining"].as_array().into_iter().flatten().filter_map(|x| x["pid"].as_u64()).map(|x| x as u32));
  }
  let (mut closed_apps, mut skipped_apps) = (Vec::new(), Vec::new());
  for p in &targets {
    let list = if left.contains(&p.pid) { &mut skipped_apps } else { &mut closed_apps };
    let a = procs::app_of(p);
    if !list.contains(&a) {
      list.push(a);
    }
  }
  closed_apps.retain(|a| !skipped_apps.contains(a));
  let trim = trim_impl(s);
  json!({
    "closedApps": closed_apps, "skippedApps": skipped_apps, "closed": closed, "trimmed": trim["trimmed"],
    "freedMB": mb_of(free_bytes() - before), "mem": system::mem_now(),
  })
}

// ---------- Commands ----------

#[tauri::command]
async fn get_state(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let marks = s.marks();
    let mut sys = s.sys.lock().unwrap();
    let procs_now = system::list_procs(&mut sys);
    let own = system::own_pids(&procs_now);
    let groups = build_groups(&procs_now, &own, &marks);
    let mem = system::mem_now();
    let recs = recommend(&procs_now, &groups, &mem);
    let live = s.cpu_live.lock().unwrap().clone();
    let home = system::home(live.total);
    let name = sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
    drop(sys);
    let (rows, err) = s.readings();
    let cpu = cpu::build(name, &s.topology, &live, s.base_mhz, rows.as_deref(), s.fans(), err);
    let battery_health = {
      let report = s.battery_report.lock().unwrap();
      battery::health(rows.as_deref(), report.as_ref().map(|(_, r)| r)).map(f64::round)
    };
    let (plugged, update_available) = {
      let st = s.settings.lock().unwrap();
      (st.plugged.clone(), update::pending(&st.updates.latest_seen))
    };
    // Shimmy itself: this process, its window engine and the sensor sidecar.
    let self_mb = mb_of(procs_now.iter().filter(|p| own.contains(&p.pid)).map(|p| p.ws as i64).sum());
    let heat = s.settings.lock().unwrap().heat.clone();
    let heat = json!({ "enabled": heat.enabled, "boostOff": heat.saved.is_some(), "since": heat.saved.map(|b| b.since), "hotC": heat::HOT_C, "coolC": heat::COOL_C });
    let fan_mode = s.atk.as_ref().and_then(|a| a.fan_mode());
    json!({ "mem": mem, "groups": groups, "marks": marks, "recs": recs, "home": home, "cpu": cpu, "batteryHealth": battery_health, "plugged": plugged, "selfMB": self_mb, "fanMode": fan_mode, "heat": heat, "updateAvailable": update_available, "cool": job_json(&s.cool), "priorities": s.settings.lock().unwrap().priorities.clone(), "at": store::now_ms() })
  })
  .await
  .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_history(s: State<'_, Shr>) -> Value {
  json!({ "points": s.store.history(), "cpu": s.store.cpu_history() })
}

/// The Graphics page (and the Processor page's summary): one entry per graphics chip, any vendor. Windows' counters
/// give per-engine and per-app use; the graphics kernel gives temperature, clocks, fan and power share for every chip
/// that may be asked (a sleeping dedicated card never is, see gpu::kernel_readouts); NVML adds NVIDIA's detail, read
/// only while a page asks and only for a card that is in use. NVML is not loaded at all otherwise.
#[tauri::command]
async fn get_gpu(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let names: HashMap<u32, String> = system::list_procs(&mut s.sys.lock().unwrap()).iter().map(|p| (p.pid, procs::app_of(p))).collect();
    let (mut adapters, apps, counters_error) = match s.gpu_counters.lock().unwrap().get_or_insert_with(gpu::Counters::open) {
      Ok(c) => {
        let (a, p) = c.sample(&names);
        (a, p, None)
      }
      Err(e) => (Vec::new(), Vec::new(), Some(e.clone())),
    };
    // NVML only for an NVIDIA card Windows' counters show in use: asking it wakes a sleeping card. Without Windows' list
    // nothing is known about the card, so NVML is not asked blindly either.
    let (nv, error) = if !gpu::nvml_wanted(&adapters) {
      (None, None)
    } else {
      let mut slot = s.nvml.lock().unwrap();
      match slot.get_or_insert_with(|| nvml_wrapper::Nvml::init().map_err(|e| e.to_string())) {
        Ok(n) => match gpu::nvidia(n) {
          Ok(g) => (Some(g), None),
          Err(e) => (None, Some(e)),
        },
        Err(e) => (None, Some(e.clone())),
      }
    };
    // The graphics kernel is asked with no lock held (it is slow, and the Battery page needs the counters), then what it
    // learned about the chips is stored.
    let learned = gpu::kernel_readouts(&mut adapters);
    if let Some(Ok(c)) = s.gpu_counters.lock().unwrap().as_mut() {
      c.learn(&learned);
    }
    gpu::readouts_without_counters(&mut adapters, nv);
    json!({ "error": error, "adapters": adapters, "apps": apps, "countersError": counters_error })
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
    let marks = s.marks();
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
          let c = classify(p, &own, &marks);
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
    let restore = gone_for_good.then(|| s.restore_point_first("deleting a folder for good"));
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
    Ok(json!({ "deleted": true, "bytes": c.bytes, "permanent": gone_for_good, "restorePoint": restore }))
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

/// Windows' own Installed apps page, for Store apps and anything the Programs page does not list.
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
    let r = if cleanup {
      progress.set("Making a restore point");
      let note = s2.restore_point_first("Windows update backups clean-up");
      clean::dism_cleanup(&progress).map(|mut d| {
        d.summary = format!("{} {note}", d.summary);
        d
      })
    } else {
      clean::dism_analyze(&progress)
    };
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
    let restore = (!big.is_empty()).then(|| s.restore_point_first("deleting files too big for the Recycle Bin"));
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
    Ok(json!({ "recycled": recycled, "recycledBytes": recycled_bytes, "permanent": permanent, "permanentBytes": permanent_bytes, "failed": failed, "refused": refused, "restorePoint": restore }))
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
    let live = system::battery_now();
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
    let (gpu_name, gpu_awake, gpu_w) = s.dgpu_power();
    let cpu_w = rows.as_deref().and_then(|r| sensors::value(r, "Cpu", "Power", "Package"));
    let total = battery::total_draw(view.plugged, view.rate_w, cpu_w, gpu_w.unwrap_or(0.0), *s.rest_w.lock().unwrap());
    let power = json!({
      "batteryW": view.rate_w,
      "cpuW": cpu_w,
      "gpuPresent": gpu_name.is_some(),
      "gpuName": gpu_name,
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
fn set_charge_limit(s: State<'_, Shr>, pct: u32, confirmed: Option<bool>) -> Result<Value, String> {
  if !(50..=100).contains(&pct) {
    return Err("The charge limit must be between 50% and 100%.".into());
  }
  if s.settings.lock().unwrap().plugged.on {
    return Err("Turn off Run from the charger first; it sets the limit itself while it is on.".into());
  }
  s.check_model(confirmed.unwrap_or(false))?;
  let result = s.write_limit(pct)?;
  s.settings.lock().unwrap().charge_limit = Some(pct);
  s.save_settings();
  s.store.log("charge-limit", &format!("{pct}%"), &format!("ASUS firmware answered {result}"));
  Ok(json!({ "ok": true, "result": result }))
}

/// Turns Run from the charger on (hold at the current charge, never below 40 %) or off (back to the 60/80/100 choice).
/// Returns the limit written.
fn plugged_mode_impl(s: &Shared, on: bool, confirmed: bool) -> Result<u32, String> {
  if !s.limit_supported() {
    return Err("This laptop's firmware does not offer a charge limit, which Run from the charger needs.".into());
  }
  if on {
    if !system::battery_now().present {
      return Err("No battery found.".into());
    }
    s.check_model(confirmed)?;
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
async fn set_plugged_mode(app: AppHandle, s: State<'_, Shr>, on: bool, confirmed: Option<bool>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let r = plugged_mode_impl(&s, on, confirmed.unwrap_or(false));
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
async fn set_fan_mode(s: State<'_, Shr>, mode: String, confirmed: Option<bool>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let atk = s.atk.as_ref().ok_or("The ASUS control interface is not available.")?;
    atk.fan_mode().ok_or("This laptop's firmware does not offer fan modes.")?;
    s.check_model(confirmed.unwrap_or(false))?;
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
  let temp = s.readings().0.and_then(|r| sensors::cpu_temp(&r));
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

/// Power mode follows the charger, every 3 s. It acts only when the charger is plugged in or pulled out (power::decide),
/// and saves the mode it found the first time, so Exit can put it back.
fn power_watch(s: &Shared) {
  let (p, boost_off) = {
    let st = s.settings.lock().unwrap();
    (st.power.clone(), st.heat.saved.is_some())
  };
  if !p.follow {
    return;
  }
  let plugged = system::battery_now().plugged;
  let Some(on_charger) = power::decide(p.follow, boost_off, plugged, *s.power_last.lock().unwrap()) else { return };
  *s.power_last.lock().unwrap() = Some(on_charger);
  let want = power::guid_of(if on_charger { &p.plugged } else { &p.battery });
  let before = power::current();
  if before == Some(want) {
    return;
  }
  let side = if on_charger { "charger plugged in" } else { "on battery" };
  match power::set(want) {
    Ok(()) => {
      {
        let mut st = s.settings.lock().unwrap();
        if st.power.saved.is_none() {
          st.power.saved = before.map(power::to_text);
        }
      }
      s.save_settings();
      s.store.log("power-mode", power::label(want), &format!("{side}; was {}", before.map(power::label).unwrap_or("unknown")));
    }
    Err(e) => s.store.log("power-mode-failed", power::label(want), &e),
  }
}

// ---------- Cooling check ----------

/// Full load on every logical processor for `seconds` (60 to 120), watched every second. Refused without a temperature
/// reading, since then it could not stop at 95 °C.
#[tauri::command]
fn start_cool_check(s: State<'_, Shr>, seconds: u32) -> Result<bool, String> {
  match s.readings().0.as_deref().and_then(sensors::cpu_temp) {
    None => return Err("The cooling check needs the processor's temperature, so it can stop at 95 °C, and Shimmy cannot read it right now.".into()),
    Some(t) if t >= cool::STOP_C - 10.0 => return Err(format!("The processor is already at {t:.0} °C. Let it cool down first.")),
    _ => {}
  }
  let limit = cool::clamp_secs(seconds) as u64 * 1000;
  // A second Start while a check runs (or is still stopping) must not clear its Stop.
  if s.cool.lock().unwrap().running {
    return Ok(false);
  }
  s.cool_stop.store(false, Ordering::SeqCst);
  let s2 = s.inner().clone();
  Ok(jobs::start(&s.cool, move |progress| {
    let started = store::now_ms();
    let load = cool::load(&s2.cool_stop);
    // The load threads stop when this goes out of scope, even if the loop below panics.
    let _stop = cool::StopOnDrop(s2.cool_stop.clone());
    let (mut samples, mut no_temp, mut boost_off) = (Vec::new(), 0u64, false);
    let stop = loop {
      std::thread::sleep(Duration::from_secs(1));
      let t_ms = store::now_ms().saturating_sub(started);
      let rows = s2.readings().0;
      let live = s2.cpu_live.lock().unwrap().clone();
      let v = cpu::build(String::new(), &s2.topology, &live, s2.base_mhz, rows.as_deref(), s2.fans(), None);
      no_temp = if v.temp_c.is_some() { 0 } else { no_temp + 1000 };
      boost_off |= s2.settings.lock().unwrap().heat.saved.is_some();
      samples.push(cool::Sample { t_ms, temp: v.temp_c, clock: v.avg_clock_mhz, watts: v.package_w, fan: v.fans.iter().map(|f| f.rpm).max() });
      let deg = v.temp_c.map(|t| format!("{t:.0} °C")).unwrap_or_else(|| "no temperature".into());
      let clk = v.avg_clock_mhz.map(|c| format!(", {:.1} GHz", c / 1000.0)).unwrap_or_default();
      progress.set(format!("{} s of {}, {deg}{clk}", t_ms / 1000, limit / 1000));
      if let Some(why) = cool::stop_reason(t_ms, limit, v.temp_c, no_temp, s2.cool_stop.load(Ordering::SeqCst)) {
        break why;
      }
    };
    s2.cool_stop.store(true, Ordering::SeqCst);
    for h in load {
      let _ = h.join();
    }
    let v = cool::verdict(&samples, stop, s2.base_mhz, boost_off);
    s2.store.log("cooling-check", &format!("{} s, peak {}", v.seconds, v.peak_c.map(|t| format!("{t:.0} °C")).unwrap_or_else(|| "unknown".into())), &v.text);
    Ok(v)
  }))
}

#[tauri::command]
fn stop_cool_check(s: State<'_, Shr>) -> bool {
  s.cool_stop.store(true, Ordering::SeqCst);
  true
}

// ---------- Programs: installed apps, updates, background priority ----------

fn uninstaller_open(s: &Shared) -> bool {
  s.uninstaller.lock().unwrap().as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
}

#[tauri::command]
async fn get_programs(s: State<'_, Shr>, installed: Option<bool>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    // The page polls every 1.5 s while winget runs; it asks for the installed list (a full registry read) only
    // when that list can have changed.
    let list = installed.unwrap_or(true).then(programs::list);
    json!({ "installed": list, "updates": job_json(&s.app_updates), "updateRun": job_json(&s.app_update_run), "uninstalling": uninstaller_open(&s) })
  })
  .await
  .map_err(|e| e.to_string())
}

#[tauri::command]
fn check_app_updates(s: State<'_, Shr>) -> bool {
  jobs::start(&s.app_updates, |progress| {
    progress.set("Asking winget");
    programs::upgrades()
  })
}

/// Updates one app that winget listed, after the page's confirm. One at a time, logged either way.
#[tauri::command]
fn start_app_update(s: State<'_, Shr>, id: String) -> Result<bool, String> {
  let listed = s.app_updates.lock().unwrap().result.clone().flatten().unwrap_or_default();
  let app = listed.into_iter().find(|u| u.id == id).ok_or("Check for updates first: Shimmy only updates apps winget listed.")?;
  if s.app_update_run.lock().unwrap().running {
    return Err("Another update is still running. They go one at a time.".into());
  }
  let s2 = s.inner().clone();
  Ok(jobs::start(&s.app_update_run, move |progress| {
    progress.set(format!("Updating {}", app.name));
    let r = programs::upgrade(&app.id);
    let target = format!("{} {}", app.name, app.available);
    match &r {
      Ok(()) => {
        s2.store.log("app-update", &target, &format!("winget upgrade --id {} --exact, from {}", app.id, app.version));
        if let Some(Some(list)) = s2.app_updates.lock().unwrap().result.as_mut() {
          list.retain(|u| u.id != app.id);
        }
      }
      Err(e) => s2.store.log("app-update-failed", &target, e),
    }
    Ok(json!({ "id": app.id, "name": app.name, "ok": r.is_ok(), "message": r.err() }))
  }))
}

/// Windows' Installed apps page. Handed to File Explorer, which passes it to the user's own shell, so Settings runs with
/// the user's rights and not Shimmy's.
fn open_installed_apps() -> Result<(), String> {
  let explorer = clean::known("SystemRoot").ok_or("Windows did not name its folder.")?.join("explorer.exe");
  std::process::Command::new(explorer).arg("ms-settings:appsfeatures").spawn().map(drop).map_err(|e| format!("Could not open Installed apps: {e}"))
}

/// Starts an installed app's own uninstaller, read from the registry for this key. Apps installed just for this user
/// open Windows' Installed apps page instead (programs::per_user). One at a time; its own window asks
/// the rest, and Shimmy does not wait for it.
#[tauri::command]
async fn uninstall_app(s: State<'_, Shr>, key: String) -> Result<Value, String> {
  use std::os::windows::process::CommandExt;
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    if programs::per_user(&key) {
      let name = programs::name_of(&key)?;
      open_installed_apps()?;
      s.store.log("uninstall", &name, "installed just for this user: opened Windows' Installed apps page");
      return Ok(json!({ "name": name, "settings": true }));
    }
    // Held from the check to the start, so two quick clicks cannot start two uninstallers.
    let mut slot = s.uninstaller.lock().unwrap();
    if slot.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None))) {
      return Err("Another uninstaller is still open. Finish that one first.".to_string());
    }
    let (name, exe, args) = programs::uninstall_command(&key)?;
    let mut cmd = std::process::Command::new(&exe);
    if !args.is_empty() {
      cmd.raw_arg(&args);
    }
    let child = cmd.spawn().map_err(|e| format!("Windows could not start its uninstaller: {e}"))?;
    *slot = Some(child);
    drop(slot);
    s.store.log("uninstall", &name, &format!("started its own uninstaller: {exe} {args}"));
    Ok(json!({ "name": name }))
  })
  .await
  .map_err(|e| e.to_string())?
}

/// Sets an app's background priority: "below", "low", "eco", or "normal" to put it back. Refused for anything Shimmy
/// protects for a built-in reason, the same gate as marking an app Safe.
#[tauri::command]
async fn set_priority(s: State<'_, Shr>, exes: Vec<String>, level: String) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let level = prio::parse_level(&level)?;
    let procs_now = system::list_procs(&mut s.sys.lock().unwrap());
    let own = system::own_pids(&procs_now);
    let mut keys = Vec::new();
    for e in exes.iter().take(50) {
      let k = procs::exe_key(e)?;
      let running_why = || procs_now.iter().filter(|p| p.name.eq_ignore_ascii_case(&k)).map(|p| classify(p, &own, &Marks::new())).find(|c| c.builtin).and_then(|c| c.reason);
      if level.is_some() {
        if let Some(why) = procs::builtin_reason(&k).or_else(running_why) {
          return Err(format!("{k} keeps its priority: {why}."));
        }
      }
      keys.push(k);
    }
    {
      let mut st = s.settings.lock().unwrap();
      // Checked before anything changes, so a refused request leaves the list as it was.
      if level.is_some() && st.priorities.len() + keys.iter().filter(|k| !st.priorities.contains_key(*k)).count() > 100 {
        return Err("Up to 100 apps can have a background priority.".into());
      }
      for k in &keys {
        match level {
          Some(l) => st.priorities.insert(k.clone(), l),
          None => st.priorities.remove(k),
        };
      }
    }
    s.save_settings();
    for k in &keys {
      s.store.log("priority", k, level.map_or("normal again", |l| l.words()));
    }
    prio_watch(&s);
    Ok(json!({ "priorities": s.settings.lock().unwrap().priorities.clone() }))
  })
  .await
  .map_err(|e| e.to_string())?
}

/// Background priority, every 30 s and when the list changes: new processes of a listed app get its level, and
/// processes of an app taken off the list are put back. Protected processes are never touched.
fn prio_watch(s: &Shared) {
  let wanted = s.settings.lock().unwrap().priorities.clone();
  let mut applied = s.prio_applied.lock().unwrap();
  if wanted.is_empty() && applied.is_empty() {
    return;
  }
  let procs_now = system::list_procs(&mut s.sys.lock().unwrap());
  let own = system::own_pids(&procs_now);
  let running: Vec<(u32, String)> = procs_now.iter().filter(|p| !classify(p, &own, &Marks::new()).builtin).map(|p| (p.pid, p.name.to_lowercase())).collect();
  // Exited processes are forgotten. Windows reuses process ids, so an entry is kept only while the same id still runs
  // the same exe: another app that got the id is then neither skipped nor "put back".
  applied.retain(|pid, a| running.iter().any(|(p, exe)| p == pid && *exe == a.0));
  let (set, back) = prio::plan(&running, &wanted, &applied);
  let (mut lowered, mut restored): (BTreeMap<String, usize>, BTreeMap<String, usize>) = Default::default();
  for (pid, level) in set {
    let exe = running.iter().find(|r| r.0 == pid).map(|r| r.1.clone()).unwrap_or_default();
    if let Ok(prev) = prio::apply(pid, level) {
      let orig = applied.get(&pid).map_or(prio::original(prev), |a| a.1);
      applied.insert(pid, (exe.clone(), orig, level));
      *lowered.entry(exe).or_default() += 1;
    }
  }
  for pid in back {
    if let Some((exe, old, _)) = applied.remove(&pid) {
      prio::revert(pid, old);
      *restored.entry(exe).or_default() += 1;
    }
  }
  for (exe, n) in lowered {
    let words = wanted.get(&exe).map_or("", |l| l.words());
    s.store.log("priority-set", &exe, &format!("{n} process{} set to {words}", if n == 1 { "" } else { "es" }));
  }
  for (exe, n) in restored {
    s.store.log("priority-restored", &exe, &format!("{n} process{} back to the priority {} had", if n == 1 { "" } else { "es" }, if n == 1 { "it" } else { "they" }));
  }
}

/// Puts every process Shimmy lowered back, at Exit.
fn restore_priorities(s: &Shared) {
  let applied: Vec<_> = s.prio_applied.lock().unwrap().drain().collect();
  if applied.is_empty() {
    return;
  }
  // Only a process that still runs the same exe: its id may have gone to another app since the last 30 s check.
  let procs_now = system::list_procs(&mut s.sys.lock().unwrap());
  let same = |pid: u32, exe: &str| procs_now.iter().any(|p| p.pid == pid && p.name.eq_ignore_ascii_case(exe));
  let back = applied.iter().filter(|(pid, (exe, old, _))| same(*pid, exe) && prio::revert(*pid, *old)).count();
  s.store.log("priority-restored", "all", &format!("{back} of {} processes put back: Shimmy exited", applied.len()));
}

// ---------- System info and exports ----------

/// The System page: what is inside this PC (hw.rs). Read once, then kept until the window closes.
#[tauri::command]
async fn get_system_info(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    if let Some(v) = s.hw_info.lock().unwrap().clone() {
      return Ok(v);
    }
    let mut v = hw::read()?;
    v["shimmy"] = json!(update::CURRENT);
    *s.hw_info.lock().unwrap() = Some(v.clone());
    Ok(v)
  })
  .await
  .map_err(|e| e.to_string())?
}

/// Local date and time for file names, such as 2026-10-29 143205.
fn local_stamp() -> String {
  let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
  format!("{:04}-{:02}-{:02} {:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Local date and time for people to read, such as 2026-10-29 14:32:05.
fn local_clock() -> String {
  let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
  format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Every socket by app. The names are the same ones the Apps page uses.
fn connections(s: &Shared) -> Result<Vec<net::AppConns>, String> {
  let conns = net::list()?;
  let procs_now = system::list_procs(&mut s.sys.lock().unwrap());
  let own = system::own_pids(&procs_now);
  let names = procs_now.iter().map(|p| (p.pid, classify(p, &own, &Marks::new()).app)).collect();
  Ok(net::by_app(conns, &names))
}

/// The Connections page: read only (net.rs).
#[tauri::command]
async fn get_connections(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || Ok(json!({ "apps": connections(&s)? }))).await.map_err(|e| e.to_string())?
}

/// The System page's Save a report, from what the running app already knows (report.rs).
#[tauri::command]
async fn save_report(app: AppHandle, s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  let dir = app.path().download_dir().map_err(|e| e.to_string())?;
  tauri::async_runtime::spawn_blocking(move || {
    let hw = s.hw_info.lock().unwrap().clone().or_else(|| hw::read().ok());
    let snap = report::Snapshot {
      when: local_clock(),
      hw,
      cpu_load: Some(s.cpu_live.lock().unwrap().total as f64),
      rows: s.readings().0,
      fans: s.fans(),
      mem: system::mem_now(),
      drives: storage::drives(),
      battery: system::battery_now(),
      uptime_s: System::uptime(),
      power_mode: power::current().map(power::label).map(String::from),
      procs: system::list_procs(&mut s.sys.lock().unwrap()),
      conns: net::list().ok(),
    };
    let (name, _) = report::save(&dir, &local_stamp(), &report::text(&snap))?;
    s.store.log("report", &name, &format!("saved to {}", dir.display()));
    Ok(json!({ "name": name }))
  })
  .await
  .map_err(|e| e.to_string())?
}

/// The sensor log's state, and what it can log with each reading's value now.
#[tauri::command]
async fn get_sensor_log(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let sources: Vec<Value> = s
      .log_rows()
      .iter()
      .map(|r| json!({ "id": senselog::key(r), "hw": r.hw, "sensor": r.sensor, "unit": senselog::unit(&r.kind), "value": r.value, "suggested": senselog::default_key(r) }))
      .collect();
    let prefs = s.settings.lock().unwrap().sensor_log.clone();
    let job = s.sensor_log.lock().unwrap();
    json!({ "job": &*job, "sources": sources, "prefs": prefs, "maxColumns": senselog::MAX_COLUMNS })
  })
  .await
  .map_err(|e| e.to_string())
}

/// Starts writing the chosen readings to a new CSV in Downloads. The page sends only sensor names and an interval;
/// Shimmy picks the file. Returns false when a log is already running.
#[tauri::command]
fn start_sensor_log(app: AppHandle, s: State<'_, Shr>, keys: Vec<String>, interval_s: u32) -> Result<bool, String> {
  use std::io::Write;
  let keys = senselog::check_keys(&keys)?;
  let secs = senselog::clamp_secs(interval_s);
  // A second Start while a log runs (or is still stopping) must not clear its Stop.
  if s.sensor_log.lock().unwrap().running {
    return Ok(false);
  }
  let name = format!("Shimmy sensor log {}.csv", local_stamp());
  let path = app.path().download_dir().map_err(|e| e.to_string())?.join(&name);
  let mut file = std::fs::File::create_new(&path).map_err(|e| format!("Could not start a log in Downloads: {e}"))?;
  // The byte-order mark makes Excel read the file as UTF-8.
  let head = format!("\u{feff}{}", senselog::header(&keys));
  file.write_all(head.as_bytes()).map_err(|e| format!("Could not start a log in Downloads: {e}"))?;
  s.settings.lock().unwrap().sensor_log = senselog::Prefs { interval_s: secs, chosen: keys.clone() };
  s.save_settings();
  s.store.log("sensor-log", &name, &format!("started: {} sensors every {secs} s", keys.len()));
  s.sensor_log_stop.store(false, Ordering::SeqCst);
  let s2 = s.inner().clone();
  Ok(jobs::start(&s.sensor_log, move |progress| {
    let (mut bytes, mut rows, mut due) = (head.len() as u64, 0u64, store::now_ms());
    let why = loop {
      if let Some(why) = senselog::stop_reason(s2.sensor_log_stop.load(Ordering::SeqCst), bytes) {
        break why;
      }
      let now = store::now_ms();
      if now >= due {
        let row = senselog::line(&local_clock(), &keys, &s2.log_rows());
        if let Err(e) = file.write_all(row.as_bytes()).and_then(|_| file.flush()) {
          let why = format!("Stopped after {rows} rows: {e}");
          s2.store.log("sensor-log", &name, &why);
          notify(&app, "Sensor log stopped", &why);
          return Err(why);
        }
        bytes += row.len() as u64;
        rows += 1;
        due = senselog::next_due(due, now, secs as u64 * 1000);
        progress.set(format!("{rows} row{}, {} KB, in {name}", if rows == 1 { "" } else { "s" }, bytes / 1024));
      }
      std::thread::sleep(Duration::from_millis(200));
    };
    s2.store.log("sensor-log", &name, &format!("stopped: {rows} rows, {} KB", bytes / 1024));
    if why == senselog::Why::Full {
      alert(&app, &s2, "Sensor log stopped", &format!("{name} reached 100 MB, so Shimmy stopped it. It is in Downloads."));
    }
    Ok(senselog::Done { name, path: path.display().to_string(), rows, bytes, why })
  }))
}

#[tauri::command]
fn stop_sensor_log(s: State<'_, Shr>) -> bool {
  s.sensor_log_stop.store(true, Ordering::SeqCst);
  true
}

/// Saves a CSV to Downloads and shows it in File Explorer: the finder's results ("finder"), a folder's biggest folders
/// ("folders": `path`, or your folder), or the whole Log ("log").
#[tauri::command]
async fn export_csv(app: AppHandle, s: State<'_, Shr>, what: String, path: Option<String>) -> Result<Value, String> {
  let s = s.inner().clone();
  let dir = app.path().download_dir().map_err(|e| e.to_string())?;
  tauri::async_runtime::spawn_blocking(move || {
    let row = store::export_row;
    let (label, body) = match what.as_str() {
      "finder" => {
        let j = s.finder.lock().unwrap();
        let f = j.result.as_ref().ok_or("Look for big and duplicate files first.")?;
        let item = |group: &str, i: &finder::Item| row(&[group, &i.name, &i.folder, &i.path, &i.bytes.to_string(), &store::iso(i.modified)]);
        let mut b = row(&["group", "name", "folder", "path", "bytes", "modified"]);
        for (n, g) in f.dups.iter().enumerate() {
          for i in &g.files {
            b += &item(&format!("duplicate set {}", n + 1), i);
          }
        }
        f.large.iter().for_each(|i| b += &item("large", i));
        f.installers.iter().for_each(|i| b += &item("old installer", i));
        ("big and duplicate files", b)
      }
      "folders" => {
        let tree = s.tree.lock().unwrap();
        let p = path.map(PathBuf::from).unwrap_or_else(|| s.profile.clone());
        let f = tree.as_ref().and_then(|t| storage::folder(t, &p)).ok_or("Measure C: on the Storage page first.")?;
        let mut b = row(&["name", "path", "bytes"]);
        f.children.iter().for_each(|c| b += &row(&[&c.name, &c.path, &c.bytes.to_string()]));
        if f.files_here > 0 {
          b += &row(&["(files directly in this folder)", &f.path, &f.files_here.to_string()]);
        }
        ("biggest folders", b)
      }
      "log" => ("log", s.store.export_log().ok_or("Nothing has been logged yet.")?),
      "connections" => {
        let mut b = row(&["app", "protocol", "state", "where", "local address", "local port", "remote address", "remote port"]);
        for a in connections(&s)? {
          for c in &a.conns {
            let place = c.place.map_or("", |p| match p {
              net::Place::ThisPc => "This PC",
              net::Place::Local => "Local network",
              net::Place::Internet => "Internet",
            });
            b += &row(&[&a.app, c.proto, c.state, place, &c.local, &c.local_port.to_string(), c.remote.as_deref().unwrap_or(""), &c.remote_port.map_or(String::new(), |p| p.to_string())]);
          }
        }
        ("connections", b)
      }
      _ => return Err("Shimmy does not know that export.".to_string()),
    };
    let name = format!("Shimmy {label} {}.csv", local_stamp());
    let file = dir.join(&name);
    // The byte-order mark makes Excel read the file as UTF-8.
    std::fs::write(&file, format!("\u{feff}{body}")).map_err(|e| format!("Could not save it to Downloads: {e}"))?;
    s.store.log("export", &name, &format!("saved to {}", dir.display()));
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", file.display())).spawn();
    Ok(json!({ "name": name }))
  })
  .await
  .map_err(|e| e.to_string())?
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
    "powerMode": power::current().map(power::label),
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

/// Every range and gate on settings, for the page's copy and for settings.json at start-up alike: the file is in the
/// user's own folder, so any program could have edited it, and Shimmy acts on it as administrator (a charge limit written
/// to the firmware, a plan GUID passed to powercfg, folders the finder searches). Out-of-range values are pulled back
/// into range; finder folders that fail `check_root` and an unreadable saved plan are dropped.
fn sanitize(st: &mut Settings) {
  auto::clamp(&mut st.alerts, &mut st.auto_clean, &mut st.weekly, &mut st.finder, &mut st.updates);
  st.caches.age_days = st.caches.age_days.clamp(1, 365);
  power::clamp(&mut st.power);
  senselog::clamp(&mut st.sensor_log);
  st.finder.extra_roots.retain(|r| check_root(Path::new(r)).is_ok());
  st.charge_limit = st.charge_limit.filter(|p| (50..=100).contains(p));
  if st.plugged.on {
    st.plugged.hold_pct = st.plugged.hold_pct.clamp(hold::FLOOR, 100);
  }
  if st.heat.saved.as_ref().is_some_and(|b| !heat::valid_scheme(&b.scheme)) {
    st.heat.saved = None;
  }
}

#[tauri::command]
async fn set_settings(s: State<'_, Shr>, settings: Value) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let mut new: Settings = serde_json::from_value(settings).map_err(|e| format!("Settings not understood: {e}"))?;
    // A folder the user just added is refused out loud; `sanitize` drops bad ones silently.
    for r in &new.finder.extra_roots {
      check_root(Path::new(r.trim()))?;
    }
    sanitize(&mut new);
    let (follow_was, follow_now, modes_changed) = {
      let mut cur = s.settings.lock().unwrap();
      let p = (cur.power.follow, new.power.follow, cur.power.plugged != new.power.plugged || cur.power.battery != new.power.battery);
      // Kept by Shimmy, not by the page.
      new.power.saved = cur.power.saved.clone();
      new.sensor_log = cur.sensor_log.clone();
      new.restore_point_at = cur.restore_point_at;
      new.charge_limit = cur.charge_limit;
      new.plugged = cur.plugged.clone();
      new.heat.saved = cur.heat.saved.clone();
      new.asus_untested_ok = cur.asus_untested_ok;
      new.app_marks = cur.app_marks.clone();
      new.priorities = cur.priorities.clone();
      new.weekly.last_run = cur.weekly.last_run;
      new.updates = auto::UpdatePrefs { auto_check: new.updates.auto_check, ..cur.updates.clone() };
      // Turning the weekly clean on, or moving it, counts from now, so a slot already in the past does not fire at once.
      let w = &new.weekly;
      if w.enabled && (!cur.weekly.enabled || w.day != cur.weekly.day || w.hour != cur.weekly.hour) {
        new.weekly.last_run = store::now_ms();
      }
      *cur = new;
      p
    };
    s.save_settings();
    if follow_was && !follow_now {
      s.restore_power("power mode follows the charger turned off");
    } else if follow_now && (!follow_was || modes_changed) {
      // Applies the new choice on the next tick instead of at the next plug change.
      *s.power_last.lock().unwrap() = None;
    }
    Ok(settings_json(&s))
  })
  .await
  .map_err(|e| e.to_string())?
}

/// Marks apps by exe name: "protected", "safe", or "default" to remove the mark. Safe is refused for anything Shimmy
/// protects for a built-in reason.
#[tauri::command]
async fn set_app_mark(s: State<'_, Shr>, exes: Vec<String>, mark: String) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    let mark = procs::parse_mark(&mark)?;
    // Running processes by exe name: (all protected for a built-in reason, why). Checked besides the name itself,
    // because a service or a client-work process is protected by where it runs, not by its name.
    let mut running: HashMap<String, (bool, Option<String>)> = HashMap::new();
    if mark == Some(procs::Mark::Safe) {
      let procs_now = system::list_procs(&mut s.sys.lock().unwrap());
      let own = system::own_pids(&procs_now);
      for p in &procs_now {
        let c = classify(p, &own, &Marks::new());
        let e = running.entry(p.name.to_lowercase()).or_insert((true, None));
        e.0 &= c.builtin;
        e.1 = e.1.take().or(c.reason);
      }
    }
    let blocked = |k: &str| procs::builtin_reason(k).or_else(|| running.get(k).filter(|e| e.0).and_then(|e| e.1.clone()));
    let changed = procs::set_marks(&mut s.settings.lock().unwrap().app_marks, &exes, mark, blocked)?;
    s.save_settings();
    let detail = match mark {
      Some(procs::Mark::Safe) => "safe: Clean now and the automatic clean may close it",
      Some(procs::Mark::Protected) => "protected: never closed and never trimmed",
      None => "default: only trimmed, never closed automatically",
    };
    for k in &changed {
      s.store.log("app-mark", k, detail);
    }
    Ok(json!({ "marks": s.marks() }))
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

// ---------- Updates ----------

fn update_json(s: &Shared) -> Value {
  let st = s.settings.lock().unwrap().updates.clone();
  json!({
    "current": update::CURRENT,
    "release": *s.update_info.lock().unwrap(),
    "download": job_json(&s.update_dl),
    "bytes": s.update_done.load(Ordering::Relaxed),
    "total": s.update_total.load(Ordering::Relaxed),
    "autoCheck": st.auto_check,
    "lastCheck": st.last_check,
  })
}

/// Asks GitHub for the latest release and keeps the answer: for the Updates page, and in settings the newest version
/// seen, which the rail marker reads. A failed check changes nothing, so the daily timer keeps trying.
fn check_update_impl(s: &Shared) -> Result<update::Release, String> {
  let rel = update::check()?;
  *s.update_info.lock().unwrap() = Some(rel.clone());
  {
    // A download of an older version than the one just found is dropped: it must never be what Install starts.
    let mut j = s.update_dl.lock().unwrap();
    if !j.running && update::download_is_stale(j.result.as_ref().map(|d| d.version.as_str()), &rel.latest) {
      if let Some(d) = j.result.take() {
        let _ = std::fs::remove_file(&d.path);
      }
    }
  }
  {
    let mut st = s.settings.lock().unwrap();
    st.updates.last_check = store::now_ms();
    st.updates.latest_seen = rel.latest.clone();
  }
  s.save_settings();
  Ok(rel)
}

#[tauri::command]
fn get_update(s: State<'_, Shr>) -> Value {
  update_json(&s)
}

#[tauri::command]
async fn check_update(s: State<'_, Shr>) -> Result<Value, String> {
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    if s.update_busy.swap(true, Ordering::SeqCst) {
      return Err("Shimmy is already checking.".to_string());
    }
    let r = check_update_impl(&s);
    s.update_busy.store(false, Ordering::SeqCst);
    r.map(|_| update_json(&s))
  })
  .await
  .map_err(|e| e.to_string())?
}

/// The daily check: one notification per new version. A failure is retried in an hour and is not logged or shown.
fn auto_update_check(app: &AppHandle, s: &Shared) {
  match check_update_impl(s) {
    Ok(rel) => {
      if auto::update_notify_due(&s.settings.lock().unwrap().updates, &rel.latest, rel.newer) {
        notify(app, &format!("Shimmy {} is available", rel.latest), "Open Shimmy and go to Updates to see what is new and install it.");
        s.settings.lock().unwrap().updates.last_notified = rel.latest;
        s.save_settings();
        let _ = app.emit("refresh", ());
      }
    }
    Err(_) => {
      s.settings.lock().unwrap().updates.last_check = auto::update_retry_stamp(store::now_ms());
      s.save_settings();
    }
  }
}

/// Downloads the installer for the release the last check found, as a background job; the page polls `get_update`.
#[tauri::command]
fn start_update_download(s: State<'_, Shr>) -> Result<bool, String> {
  let rel = s.update_info.lock().unwrap().clone().ok_or("Check for updates first.")?;
  if !rel.newer {
    return Err("You already have the newest version.".into());
  }
  if let Some(why) = &rel.problem {
    return Err(why.clone());
  }
  {
    let mut j = s.update_dl.lock().unwrap();
    if !j.running {
      if let Some(old) = j.result.take() {
        let _ = std::fs::remove_file(&old.path);
      }
    }
  }
  let s2 = s.inner().clone();
  Ok(jobs::start(&s.update_dl, move |progress| {
    progress.set("Downloading");
    let r = update::download(&rel, &s2.update_done, &s2.update_total);
    let target = format!("Shimmy {}", rel.latest);
    match &r {
      Ok(d) => s2.store.log("update-download", &target, &format!("{} MB, SHA-256 matched, saved to {}", d.bytes / 1_048_576, d.path.display())),
      Err(e) => s2.store.log("update-download", &target, &format!("not installed: {e}")),
    }
    r
  }))
}

/// Starts the downloaded installer and quits Shimmy the way the tray's Exit does. The installer is only started if it is
/// the version the last check found and its hash still matches.
#[tauri::command]
async fn install_update(app: AppHandle, s: State<'_, Shr>) -> Result<Value, String> {
  if app.config().identifier.ends_with(".dev") {
    return Err("Updates can only be installed from the real Shimmy, not a development build.".into());
  }
  let s = s.inner().clone();
  tauri::async_runtime::spawn_blocking(move || {
    if s.busy.load(Ordering::SeqCst) {
      return Err("A clean-up is still running. Install the update when it has finished.".to_string());
    }
    // Only the version the last check found, never whatever an earlier download left behind.
    let latest = s.update_info.lock().unwrap().as_ref().map(|r| r.latest.clone());
    let d = s.update_dl.lock().unwrap().result.clone();
    update::install_check(d.as_ref().map(|d| d.version.as_str()), latest.as_deref())?;
    let d = d.ok_or("Download the update first.")?;
    update::install(&d)?;
    s.store.log("update-install", &format!("Shimmy {}", d.version), &format!("started {}; Shimmy exits so the installer can replace it", d.path.display()));
    // Let the page get the answer before the window goes.
    std::thread::spawn(move || {
      std::thread::sleep(Duration::from_millis(500));
      exit_app(&app);
    });
    Ok(json!({ "ok": true }))
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
      format!("No apps are marked Safe to close. Trimmed {trimmed} apps.")
    } else {
      format!("Closed {}. Trimmed {trimmed} apps.", list("closedApps"))
    };
    if !list("skippedApps").is_empty() {
      body += &format!(" Still open after Shimmy asked: {}.", list("skippedApps"));
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

/// Exit from the tray, and after an update starts its installer: puts heat protection's boost back, saves, quits.
fn exit_app(app: &AppHandle) {
  let s = app.state::<Shr>();
  s.restore_boost("Shimmy exited");
  s.restore_power("Shimmy exited");
  restore_priorities(&s);
  // The log flushes every row, so this only lets it write its closing line to the Log.
  s.sensor_log_stop.store(true, Ordering::SeqCst);
  for _ in 0..20 {
    if !s.sensor_log.lock().unwrap().running {
      break;
    }
    std::thread::sleep(Duration::from_millis(50));
  }
  s.store.save();
  app.exit(0);
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
          match plugged_mode_impl(&s, on, false) {
            Ok(limit) if on => notify(&app, "Running from the charger", &format!("The battery stays at {limit}% while plugged in; the charger powers the laptop.")),
            Ok(limit) => notify(&app, "Charging normally again", &format!("Charge limit back to {limit}%.")),
            Err(e) if e.starts_with(UNTESTED) => notify(&app, "Run from the charger", "Shimmy's ASUS controls were not tested on this laptop model. Turn it on from the Battery page once to read the warning and confirm."),
            Err(e) => notify(&app, "Run from the charger", &e),
          }
          sync_tray_check(&app, &s);
          let _ = app.emit("refresh", ());
        });
      }
      "open" => show_main(app),
      "exit" => exit_app(app),
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

/// Alerts, the automatic memory clean, the weekly clean and power mode. Called by the sampler every 3 s.
fn automation(app: &AppHandle, s: &Shr, tick: u64) {
  const MIN: u64 = 60_000;
  let now = store::now_ms();
  let st = s.settings.lock().unwrap().clone();
  let m = system::mem_now();
  plugged_watch(app, s, now);
  heat_watch(s, now);
  power_watch(s);
  if tick % 10 == 5 {
    prio_watch(s);
  }
  let mut w = s.watch.lock().unwrap();
  if st.alerts.enabled {
    let a = &st.alerts;
    if w.ram.step(m.pct >= a.ram_pct as u64, now, MIN, 30 * MIN) {
      alert(app, s, "Memory is nearly full", &format!("{}% in use for over a minute. Open Shimmy to see what to close.", m.pct));
    }
    let temp = s.readings().0.and_then(|r| sensors::cpu_temp(&r));
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
  // The junk alert measures the Clean page's ticked categories hourly, on its own thread: it walks the temp folders.
  if st.alerts.enabled && st.alerts.junk_gb > 0 && tick % 1200 == 300 && !s.clean_run.lock().unwrap().running {
    let (app, s, gb) = (app.clone(), s.clone(), st.alerts.junk_gb);
    std::thread::spawn(move || {
      let cats = clean::scan(&s.running(), &jobs::Progress::new(|_| {}));
      // Only what Clean would really clean now: a cache whose app is open is skipped there, so it is not counted here.
      let bytes: u64 = cats.iter().filter(|c| c.default_on && c.blocked.is_none()).map(|c| c.bytes).sum();
      if s.watch.lock().unwrap().junk.step(auto::junk_over(bytes, gb), store::now_ms(), 0, 24 * 60 * MIN) {
        alert(&app, &s, "Junk to clean", &format!("{} of temporary files and caches can be cleaned. The Clean page shows what, and cleans it when you say.", fmt_mb((bytes / 1_048_576) as i64)));
      }
    });
  }
  // The daily update check runs on its own thread: it waits on the network.
  if tick % 20 == 15 && auto::update_due(&st.updates, now) && !s.update_busy.swap(true, Ordering::SeqCst) {
    let (app, s) = (app.clone(), s.clone());
    std::thread::spawn(move || {
      auto_update_check(&app, &s);
      s.update_busy.store(false, Ordering::SeqCst);
    });
  }
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
    // The one System whose processor usage is ever refreshed, and the one query for effective clocks.
    let mut cpu_sys = System::new();
    cpu_sys.refresh_cpu_all();
    let mut perf = cpu::Perf::open().ok();
    std::thread::sleep(Duration::from_millis(500));
    for tick in 0u64.. {
      update_tray(&app, &last);
      let s = app.state::<Shr>();
      automation(&app, s.inner(), tick);
      s.sample_power();
      if tick % 600 == 0 {
        s.reapply_charge_limit();
      }
      cpu_sys.refresh_cpu_usage();
      {
        let clocks = perf.as_mut().map(|p| p.sample()).unwrap_or_default();
        let mut live = s.cpu_live.lock().unwrap();
        *live = cpu::Live { total: cpu_sys.global_cpu_usage(), per_cpu: cpu_sys.cpus().iter().map(|c| c.cpu_usage()).collect(), clocks };
      }
      if tick % 3 == 0 {
        s.store.sample(system::mem_now().used_mb);
        let load = s.cpu_live.lock().unwrap().total;
        let temp = s.readings().0.and_then(|r| sensors::cpu_temp(&r));
        s.store.sample_cpu(load, temp.map(|t| t as f32));
      }
      if tick % 20 == 19 {
        s.store.save();
      }
      std::thread::sleep(Duration::from_secs(3));
    }
  });
}

/// `--report`: a one-page summary of this PC (report.rs). Returns the exit code, or None to start normally.
pub fn report_step() -> Option<i32> {
  std::env::args().any(|a| a == "--report").then(report::cli)
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
      let mut settings: Settings = std::fs::read_to_string(&settings_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
      sanitize(&mut settings);
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
      let model = asus::model();
      let topology = cpu::topology().unwrap_or_else(|| cpu::Topology::single_threads(sys.cpus().len()));
      sensors::start(app.path().resource_dir()?.join("sensors").join("shimmy-sensors.exe"), sensor_state.clone());
      app.manage::<Shr>(Arc::new(Shared {
        sys: Mutex::new(sys),
        store,
        busy: AtomicBool::new(false),
        sensors: sensor_state,
        atk: asus::Atk::open(),
        nvml: Mutex::new(None),
        base_mhz: cpu::base_mhz(),
        topology,
        cpu_live: Mutex::new(cpu::Live::default()),
        model_tested: asus::is_tested(&model),
        model,
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
        update_info: Mutex::new(None),
        update_dl: Default::default(),
        update_done: AtomicU64::new(0),
        update_total: AtomicU64::new(0),
        update_busy: AtomicBool::new(false),
        power_last: Mutex::new(None),
        cool: Default::default(),
        cool_stop: Arc::new(AtomicBool::new(true)),
        hw_info: Mutex::new(None),
        app_updates: Default::default(),
        app_update_run: Default::default(),
        uninstaller: Mutex::new(None),
        prio_applied: Mutex::new(prio::Applied::new()),
        sensor_log: Default::default(),
        sensor_log_stop: Arc::new(AtomicBool::new(true)),
        settings: Mutex::new(settings),
        settings_path,
      }));
      // Shimmy was closed (or the PC restarted) while it had boost off: put it back before anything else.
      app.state::<Shr>().restore_boost("Shimmy was closed while boost was off");
      // Same for the power mode, unless it is still meant to follow the charger: then the first tick sets it anyway.
      if !app.state::<Shr>().settings.lock().unwrap().power.follow {
        app.state::<Shr>().restore_power("Shimmy was closed while it had changed the power mode");
      }
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
    .invoke_handler(tauri::generate_handler![get_state, get_history, get_gpu, get_storage, start_usage_scan, storage_folder, open_path, get_clean, start_clean_scan, start_clean, start_dism, start_finder, finder_delete, get_battery, set_charge_limit, set_fan_mode, set_heat_protection, get_settings, set_settings, set_app_mark, set_autostart, set_plugged_mode, start_cache_scan, start_cache_clean, folder_preview, folder_delete, empty_bin, set_hibernation, get_restore_points, open_apps_settings, get_log, get_startup, toggle_startup, close_procs, trim, quick_clean, get_update, check_update, start_update_download, install_update, get_system_info, export_csv, start_cool_check, stop_cool_check, get_programs, check_app_updates, start_app_update, uninstall_app, set_priority, get_connections, save_report, get_sensor_log, start_sensor_log, stop_sensor_log])
    .build(tauri::generate_context!())
    .expect("error while running Shimmy Optimizer")
    .run(|_app, e| {
      // With no window open Shimmy keeps running in the tray; only the tray's Exit (an explicit exit code) quits.
      if let tauri::RunEvent::ExitRequested { api, code: None, .. } = e {
        api.prevent_exit();
      }
    });
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn settings_from_the_file_are_checked_like_the_page() {
    let mut st: Settings = serde_json::from_value(json!({
      "chargeLimit": 20,
      "plugged": { "on": true, "holdPct": 5 },
      "heat": { "saved": { "scheme": "/hibernate off", "ac": 2, "dc": 1, "since": 0 } },
      "finder": { "extraRoots": [r"C:\Windows", r"C:\"] },
      "weekly": { "day": 40, "cats": ["recycle", "nonsense"] },
      "power": { "plugged": "turbo" },
    }))
    .unwrap();
    sanitize(&mut st);
    assert_eq!(st.charge_limit, None, "never written to the firmware out of range");
    assert_eq!(st.plugged.hold_pct, hold::FLOOR);
    assert!(st.heat.saved.is_none(), "not a plan GUID, so never passed to powercfg");
    assert!(st.finder.extra_roots.is_empty(), "Windows and the whole of C: are not searched");
    assert_eq!((st.weekly.day, st.weekly.cats.clone()), (6, vec!["recycle".to_string()]));
    assert_eq!(st.power.plugged, "performance");
    let mut off = Settings::default();
    sanitize(&mut off);
    assert_eq!(off.plugged.hold_pct, 0, "an unused hold level is left alone");
  }
}
