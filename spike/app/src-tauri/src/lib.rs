use serde_json::{json, Value};

fn is_elevated() -> bool {
  use windows::Win32::Foundation::{CloseHandle, HANDLE};
  use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
  use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
  unsafe {
    let mut token = HANDLE::default();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
      return false;
    }
    let mut elev = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    let ok = GetTokenInformation(
      token,
      TokenElevation,
      Some(&mut elev as *mut _ as *mut _),
      std::mem::size_of::<TOKEN_ELEVATION>() as u32,
      &mut len,
    )
    .is_ok();
    let _ = CloseHandle(token);
    ok && elev.TokenIsElevated != 0
  }
}

fn gpu() -> Value {
  use nvml_wrapper::{enum_wrappers::device::TemperatureSensor, Nvml};
  let nvml = match Nvml::init() {
    Ok(n) => n,
    Err(e) => return json!({ "ok": false, "error": e.to_string() }),
  };
  match nvml.device_by_index(0) {
    Err(e) => json!({ "ok": false, "error": e.to_string() }),
    Ok(d) => json!({
      "ok": true,
      "name": d.name().ok(),
      "tempC": d.temperature(TemperatureSensor::Gpu).ok(),
      "loadPct": d.utilization_rates().ok().map(|u| u.gpu),
      "vramUsedMB": d.memory_info().ok().map(|m| m.used / 1_048_576),
    }),
  }
}

fn system() -> Value {
  use sysinfo::System;
  let mut s = System::new_all();
  std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
  s.refresh_cpu_all();
  json!({
    "cpu": s.cpus().first().map(|c| c.brand().trim().to_string()),
    "threads": s.cpus().len(),
    "loadPct": s.global_cpu_usage(),
    "clockMHz": s.cpus().first().map(|c| c.frequency()),
    "memTotalMB": s.total_memory() / 1_048_576,
    "memUsedMB": s.used_memory() / 1_048_576,
    "processes": s.processes().len(),
  })
}

#[tauri::command]
fn spike_report() -> Value {
  json!({ "elevated": is_elevated(), "gpu": gpu(), "system": system(), "at": chrono_now() })
}

/// Called by the page once it has rendered, so a report proves the elevated WebView2 window really drew.
#[tauri::command]
fn finish(app: tauri::AppHandle, report: Value) {
  if let Some(path) = std::env::args().skip_while(|a| a != "--report").nth(1) {
    let _ = std::fs::write(path, serde_json::to_string_pretty(&report).unwrap_or_default());
    app.exit(0);
  }
}

fn chrono_now() -> u64 {
  std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .invoke_handler(tauri::generate_handler![spike_report, finish])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
