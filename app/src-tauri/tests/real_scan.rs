//! Read-only runs against this PC's real folders. Ignored by default; run with
//! `cargo test --test real_scan -- --ignored --nocapture`. Nothing is deleted.
use shimmy_lib::jobs::Progress;
use shimmy_lib::{clean, finder, storage};
use std::path::PathBuf;
use std::time::Instant;

fn home() -> PathBuf {
  PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
}

fn gb(b: u64) -> String {
  format!("{:.1} GB", b as f64 / 1_073_741_824.0)
}

#[test]
#[ignore]
fn measure_c() {
  let t = Instant::now();
  let (u, tree) = storage::scan_usage(home(), Progress::new(|_| {}));
  println!("measured {} files in {:.0?}; used {} of {}", u.files, t.elapsed(), gb(u.used), gb(u.total));
  for c in &u.categories {
    println!("  {:<22} {}", c.name, gb(c.bytes));
  }
  let top = storage::folder(&tree, &home()).unwrap();
  for k in top.children.iter().take(8) {
    println!("    {:<30} {}", k.name, gb(k.bytes));
  }
}

#[test]
#[ignore]
fn find_files() {
  let h = home();
  let roots: Vec<PathBuf> = ["Downloads", "Desktop", "Documents", "Videos", "Pictures"].iter().map(|d| h.join(d)).filter(|p| p.exists()).collect();
  let t = Instant::now();
  let f = finder::scan(&roots, &finder::Rules::default(), &Progress::new(|_| {}));
  println!("looked at {} files in {:.0?}; skipped {} project folders", f.scanned, t.elapsed(), f.skipped_projects.len());
  for p in f.skipped_projects.iter().take(10) {
    println!("  skipped project: {p}");
  }
  println!("large: {}", f.large.len());
  for i in f.large.iter().take(6) {
    println!("  {}  {}", gb(i.bytes), i.path);
  }
  println!("old installers: {}", f.installers.len());
  for i in f.installers.iter().take(6) {
    println!("  {}  {}", gb(i.bytes), i.path);
  }
  let wasted: u64 = f.dups.iter().map(|g| g.bytes * (g.files.len() as u64 - 1)).sum();
  println!("duplicate groups: {} ({} in extra copies)", f.dups.len(), gb(wasted));
  for g in f.dups.iter().take(5) {
    println!("  {} x{}: {}", gb(g.bytes), g.files.len(), g.files.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(" | "));
  }
  // Every item offered must pass the deletion gate, and nothing from the exclusions may appear.
  for i in f.large.iter().chain(&f.installers).chain(f.dups.iter().flat_map(|g| &g.files)) {
    assert!(finder::allowed(std::path::Path::new(&i.path), &roots, &finder::Rules::default()).is_ok(), "offered but not allowed: {}", i.path);
    let lower = i.path.to_lowercase();
    assert!(!lower.contains("salon") && !lower.contains("shaila"), "client work offered: {}", i.path);
  }
}

#[test]
#[ignore]
fn clean_sizes() {
  for c in clean::scan(&Default::default(), &Progress::new(|_| {})) {
    println!("  {:<34} {:>10}  {} files{}", c.title, gb(c.bytes), c.files, c.blocked.map(|b| format!("  ({b})")).unwrap_or_default());
  }
}

#[test]
#[ignore]
fn profile_children() {
  let (_, tree) = storage::scan_usage(home(), Progress::new(|_| {}));
  let f = storage::folder(&tree, &home()).unwrap();
  println!("{} children of the profile", f.children.len());
  for k in &f.children {
    println!("  {:<40} {}", k.name, gb(k.bytes));
  }
}

#[test]
#[ignore]
fn asus_reads() {
  let atk = shimmy_lib::asus::Atk::open().expect("ATKACPI device");
  for (name, id) in [("charge limit", shimmy_lib::asus::CHARGE_LIMIT), ("cpu fan", shimmy_lib::asus::CPU_FAN), ("gpu fan", shimmy_lib::asus::GPU_FAN)] {
    println!("{name:<14} DSTS -> {:?}", atk.get(id).map(|v| format!("0x{v:08X}")));
  }
}

/// Windows' GPU counters on this PC: both adapters named, engines read. Does not touch NVML, so the RTX may stay asleep.
#[test]
#[ignore]
fn real_gpu_counters() {
  let mut c = shimmy_lib::gpu::Counters::open().expect("counters");
  let (adapters, apps) = c.sample(&Default::default());
  println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "adapters": adapters, "apps": apps })).unwrap());
  assert!(!adapters.is_empty());
}

/// The graphics kernel's readout (temperature, clocks, fan, power share) for every adapter, any vendor. Read only, and
/// like the Graphics page it skips a dedicated card that Windows shows idle, so the RTX may stay asleep.
#[test]
#[ignore]
fn real_d3dkmt() {
  let mut c = shimmy_lib::gpu::Counters::open().expect("counters");
  let report = shimmy_lib::gpu::kmt_report(&mut c);
  for (name, asleep, kind, values) in &report {
    println!("{name}: asleep {asleep}, adapter type {:?}", kind.map(|k| format!("{k:#x}")));
    println!("  {values:?}");
  }
  assert!(!report.is_empty());
  // A built-in chip can always be asked, so at least one adapter must answer.
  assert!(report.iter().any(|r| r.3.is_some()), "no adapter answered the graphics kernel");
}

#[test]
#[ignore]
fn real_nvidia() {
  let nvml = nvml_wrapper::Nvml::init().expect("nvml");
  println!("{:#?}", shimmy_lib::gpu::nvidia(&nvml).expect("gpu"));
}

/// P3 Step 1: Windows' connection tables, to compare with `netstat -ano`.
#[test]
#[ignore]
fn connections() {
  let t = Instant::now();
  let conns = shimmy_lib::net::list().expect("tables");
  println!("{} sockets in {:?}", conns.len(), t.elapsed());
  let count = |proto: &str, state: &str| conns.iter().filter(|c| c.proto == proto && c.state == state).count();
  println!("TCP connected {}, listening {}, closing {}, connecting {}, UDP {}", count("TCP", "Connected"), count("TCP", "Listening"), count("TCP", "Closing"), count("TCP", "Connecting"), count("UDP", "Open"));
  for c in conns.iter().filter(|c| c.state == "Connected").take(8) {
    println!("{c:?}");
  }
  assert!(!conns.is_empty());
}

/// Phase 7 firmware test: writes the charge limit from SHIMMY_LIMIT (the same call as the Battery page buttons).
/// Changes a firmware setting, so it is only run by hand: SHIMMY_LIMIT=61 cargo test --test real_scan set_limit -- --ignored
#[test]
#[ignore]
fn set_limit() {
  let pct: u32 = std::env::var("SHIMMY_LIMIT").expect("SHIMMY_LIMIT").parse().unwrap();
  let atk = shimmy_lib::asus::Atk::open().expect("ATKACPI");
  println!("DSTS before: {:?}", atk.get(shimmy_lib::asus::CHARGE_LIMIT).map(|v| format!("{v:#x}")));
  println!("DEVS {pct} answered: {:?}", atk.set(shimmy_lib::asus::CHARGE_LIMIT, pct));
}
