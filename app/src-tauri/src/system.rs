//! Windows calls: process list, memory, closing, trimming, and the Home readouts.
use crate::procs::{Mem, Proc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use windows::core::{w, BOOL};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
  GetExitCodeProcess, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
  EnumWindows, GetWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, GW_OWNER, WM_CLOSE,
};

const MB: f64 = 1_048_576.0;
const STILL_ACTIVE: u32 = 259;

pub fn mem_now() -> Mem {
  let mut s = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
  unsafe {
    let _ = GlobalMemoryStatusEx(&mut s);
  }
  let (total, free) = (s.ullTotalPhys, s.ullAvailPhys);
  let mb = |b: u64| (b as f64 / MB).round() as u64;
  Mem { total_mb: mb(total), free_mb: mb(free), used_mb: mb(total - free), pct: ((total - free) as f64 / total.max(1) as f64 * 100.0).round() as u64 }
}

/// Visible, unowned top-level windows with a title, as (pid, hwnd, title). Close to .NET's MainWindowTitle.
fn top_windows() -> Vec<(u32, isize, String)> {
  unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
    unsafe {
      let out = &mut *(lp.0 as *mut Vec<(u32, isize, String)>);
      let unowned = GetWindow(hwnd, GW_OWNER).map(|h| h.0.is_null()).unwrap_or(true);
      if IsWindowVisible(hwnd).as_bool() && unowned {
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
          let mut buf = vec![0u16; len as usize + 1];
          let n = GetWindowTextW(hwnd, &mut buf);
          let mut pid = 0u32;
          GetWindowThreadProcessId(hwnd, Some(&mut pid));
          out.push((pid, hwnd.0 as isize, String::from_utf16_lossy(&buf[..n.max(0) as usize])));
        }
      }
      BOOL(1)
    }
  }
  let mut out: Vec<(u32, isize, String)> = Vec::new();
  unsafe {
    let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize));
  }
  out
}

fn session_of(pid: u32) -> Option<u32> {
  let mut sid = 0u32;
  unsafe { ProcessIdToSessionId(pid, &mut sid).ok().map(|_| sid) }
}

pub fn list_procs(sys: &mut System) -> Vec<Proc> {
  sys.refresh_processes_specifics(
    ProcessesToUpdate::All,
    true,
    ProcessRefreshKind::nothing().with_memory().with_cpu().with_cmd(UpdateKind::OnlyIfNotSet).with_exe(UpdateKind::OnlyIfNotSet),
  );
  let threads = sys.cpus().len().max(1) as f32;
  let mut titles: HashMap<u32, String> = HashMap::new();
  for (pid, _, title) in top_windows() {
    titles.entry(pid).or_insert(title);
  }
  sys
    .processes()
    .values()
    .map(|p| {
      let pid = p.pid().as_u32();
      Proc {
        pid,
        ppid: p.parent().map(|x| x.as_u32()).unwrap_or(0),
        name: p.name().to_string_lossy().into_owned(),
        ws: p.memory(),
        cmd: p.cmd().iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "),
        path: p.exe().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
        title: titles.get(&pid).cloned().unwrap_or_default(),
        started: Some(p.start_time()).filter(|t| *t > 0),
        session: session_of(pid),
        cpu: p.cpu_usage() / threads,
      }
    })
    .collect()
}

/// Shimmy's own pid plus every descendant (its WebView2 processes, later the sensor sidecar).
pub fn own_pids(procs: &[Proc]) -> HashSet<u32> {
  let mut own: HashSet<u32> = [std::process::id()].into();
  loop {
    let before = own.len();
    for p in procs {
      if own.contains(&p.ppid) && p.pid != p.ppid {
        own.insert(p.pid);
      }
    }
    if own.len() == before {
      return own;
    }
  }
}

fn alive(pid: u32) -> bool {
  unsafe {
    let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
    let mut code = 0u32;
    let running = GetExitCodeProcess(h, &mut code).is_ok() && code == STILL_ACTIVE;
    let _ = CloseHandle(h);
    running
  }
}

/// Graceful asks each app's main window to close and waits 5 s; force terminates. Returns the pids still running.
pub fn close(pids: &[u32], force: bool) -> Vec<u32> {
  let set: HashSet<u32> = pids.iter().copied().collect();
  unsafe {
    if force {
      for &pid in pids {
        if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
          let _ = TerminateProcess(h, 1);
          let _ = CloseHandle(h);
        }
      }
      std::thread::sleep(Duration::from_millis(500));
    } else {
      for (pid, hwnd, _) in top_windows() {
        if set.contains(&pid) {
          let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
      }
      std::thread::sleep(Duration::from_millis(5500));
    }
  }
  pids.iter().copied().filter(|&p| alive(p)).collect()
}

/// Empties the working set of every process it can open, except those in `skip`. Returns how many were trimmed.
pub fn trim_all(sys: &System, skip: &HashSet<u32>) -> u32 {
  let mut n = 0;
  for pid in sys.processes().keys() {
    if skip.contains(&pid.as_u32()) {
      continue;
    }
    unsafe {
      if let Ok(h) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION, false, pid.as_u32()) {
        if EmptyWorkingSet(h).is_ok() {
          n += 1;
        }
        let _ = CloseHandle(h);
      }
    }
  }
  n
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
  pub present: bool,
  pub pct: Option<u8>,
  pub plugged: bool,
  /// Windows says the battery is taking charge.
  pub charging: bool,
  /// Seconds left on battery, when Windows knows.
  pub seconds_left: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Home {
  pub cpu_pct: f32,
  pub disk_free_gb: f64,
  pub disk_total_gb: f64,
  pub battery: Battery,
}

/// `cpu_pct` is the sampler's latest reading; only the sampler refreshes processor use.
pub fn home(cpu_pct: f32) -> Home {
  let (mut free, mut total) = (0u64, 0u64);
  unsafe {
    let _ = GetDiskFreeSpaceExW(w!("C:\\"), None, Some(&mut total), Some(&mut free));
  }
  Home {
    cpu_pct,
    disk_free_gb: free as f64 / 1_073_741_824.0,
    disk_total_gb: total as f64 / 1_073_741_824.0,
    battery: battery_now(),
  }
}

/// The last known charger state, used while Windows reports it as unknown (255) so a blip does not look like an unplug.
static LAST_PLUGGED: AtomicBool = AtomicBool::new(false);

pub fn battery_now() -> Battery {
  let mut ps = SYSTEM_POWER_STATUS::default();
  let have = unsafe { GetSystemPowerStatus(&mut ps).is_ok() };
  let present = have && ps.BatteryFlag != 128 && ps.BatteryFlag != 255;
  Battery {
    present,
    pct: (present && ps.BatteryLifePercent <= 100).then_some(ps.BatteryLifePercent),
    plugged: match ps.ACLineStatus {
      255 => LAST_PLUGGED.load(Ordering::Relaxed),
      s => {
        LAST_PLUGGED.store(s == 1, Ordering::Relaxed);
        s == 1
      }
    },
    charging: present && ps.BatteryFlag & 8 != 0,
    seconds_left: (present && ps.BatteryLifeTime != u32::MAX).then_some(ps.BatteryLifeTime),
  }
}

/// Hands freed heap back to Windows and trims Shimmy's own working set.
pub fn compact_self() {
  use windows::Win32::System::Memory::{GetProcessHeap, HeapCompact, HEAP_FLAGS};
  use windows::Win32::System::Threading::GetCurrentProcess;
  unsafe {
    if let Ok(h) = GetProcessHeap() {
      let _ = HeapCompact(h, HEAP_FLAGS(0));
    }
    let _ = EmptyWorkingSet(GetCurrentProcess());
  }
}
