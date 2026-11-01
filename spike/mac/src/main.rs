//! mac-probe: Phase 1 of the Mac port (PLAN_mac.md). It finds out what really works on a Mac before more is built.
//!
//! READ ONLY. It runs without root and never asks for it. The only things it writes are the two report files in the
//! current folder and a fresh folder under the system temp folder, which it removes at the end. It never writes to the
//! SMC, never changes a setting, and deletes only inside its own temp folder, plus the two uniquely named test files it
//! sends to the Trash itself. External tools are run by absolute path only, each with a time limit.
//!
//! Each check records a name, a result (WORKS, PARTIAL, FAILS or NOT PRESENT), the evidence, and the kind of machine it
//! ran on. One check failing never stops the others. The report is public (a GitHub artifact), so it holds counts and
//! ports and never user names, file names, host names, serial numbers or remote addresses.
//!
//! On any other system the program only says so, which keeps it building on Windows.

#[cfg(not(target_os = "macos"))]
fn main() {
  println!("mac-probe only runs on macOS. Nothing was checked and nothing was written.");
}

#[cfg(target_os = "macos")]
fn main() {
  probe::run();
}

#[cfg(target_os = "macos")]
mod probe {
  use serde_json::{json, Value};
  use std::collections::{BTreeMap, BTreeSet};
  use std::ffi::{c_char, c_int, c_void, CStr, CString};
  use std::fmt::Write as _;
  use std::fs;
  use std::io::Read;
  use std::os::macos::fs::MetadataExt as MacMeta;
  use std::os::unix::ffi::OsStrExt;
  use std::os::unix::fs::{symlink, DirBuilderExt, MetadataExt};
  use std::panic::{catch_unwind, AssertUnwindSafe};
  use std::path::{Path, PathBuf};
  use std::process::{Command, Stdio};
  use std::sync::mpsc;
  use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
  use sysinfo::{Disks, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

  const REPORT_MD: &str = "mac-probe-report.md";
  const REPORT_JSON: &str = "mac-probe-report.json";
  const NOTES: [&str; 4] = [
    "Read only: no root, no SMC writes, no settings changed. The only deletes are inside this run's own temp folder and the two uniquely named test files it sent to the Trash.",
    "GitHub's macOS runners are virtual machines. Their privacy (TCC) settings, sensors, battery and logged-in session can differ from a normal Mac, so a result from one proves little about a real Mac. The machine line on every check says which kind it ran on.",
    "Permission prompts (Desktop, Documents, Downloads, Finder automation) can appear on a normal Mac when this runs. A prompt that is never answered shows up as a timeout.",
    "Counts and ports only: no user names, file names, host names, serial numbers or remote addresses are recorded.",
  ];

  // ---------------------------------------------------------------------------------------------------------------
  // Results
  // ---------------------------------------------------------------------------------------------------------------

  #[derive(Clone, Copy, PartialEq, Eq)]
  enum Verdict {
    Works,
    Partial,
    Fails,
    NotPresent,
  }
  use Verdict::*;

  impl Verdict {
    fn label(self) -> &'static str {
      match self {
        Works => "WORKS",
        Partial => "PARTIAL",
        Fails => "FAILS",
        NotPresent => "NOT PRESENT",
      }
    }
  }

  type Out = (Verdict, String);
  /// What a check returns. An `Err` is a check that could not run at all, which counts as FAILS.
  type Res = Result<Out, Box<dyn std::error::Error>>;

  struct Check {
    name: String,
    result: Verdict,
    evidence: String,
    machine: String,
  }

  struct Report {
    machine: String,
    checks: Vec<Check>,
    /// Text that must never reach the report (the home folder, the temp folders) and what stands in for it.
    scrubs: Vec<(String, String)>,
  }

  impl Report {
    fn new(machine: String) -> Report {
      let mut r = Report { machine, checks: Vec::new(), scrubs: Vec::new() };
      if let Ok(home) = std::env::var("HOME") {
        r.scrub_path(Path::new(&home), "~");
      }
      r.scrub_path(&std::env::temp_dir(), "<tmpdir>");
      r
    }

    /// Replaces this path, and the path it really leads to, in everything added from now on.
    fn scrub_path(&mut self, path: &Path, with: &str) {
      for p in [Some(path.to_path_buf()), fs::canonicalize(path).ok()].into_iter().flatten() {
        let text = p.to_string_lossy().trim_end_matches('/').to_string();
        if text.len() > 3 {
          self.scrubs.push((text, with.to_string()));
        }
      }
    }

    fn scrub(&self, text: &str) -> String {
      let mut pairs = self.scrubs.clone();
      pairs.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
      pairs.iter().fold(text.to_string(), |t, (from, to)| t.replace(from.as_str(), to))
    }

    fn add(&mut self, name: &str, result: Verdict, evidence: impl Into<String>) {
      let evidence: String = evidence.into();
      let evidence = self.scrub(&evidence);
      eprintln!("  {:<12} {name}", result.label());
      self.checks.push(Check { name: name.to_string(), result, evidence, machine: self.machine.clone() });
    }

    /// Runs one check. A crash or an error in it becomes a FAILS row, and the next check still runs.
    fn check(&mut self, name: &str, f: impl FnOnce() -> Res) {
      let (result, evidence) = match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => (Fails, format!("the check could not run: {e}")),
        Err(p) => (Fails, format!("the check crashed: {}", panic_text(&p))),
      };
      self.add(name, result, evidence);
    }
  }

  fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "no message".into())
  }

  /// Runs a whole section; if the section's own glue crashes, that is one FAILS row and the run goes on.
  fn section(r: &mut Report, name: &str, f: fn(&mut Report)) {
    eprintln!("{name}");
    if let Err(p) = catch_unwind(AssertUnwindSafe(|| f(r))) {
      r.add(&format!("{name}: section"), Fails, format!("the section crashed: {}", panic_text(&p)));
    }
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Helpers: external tools, sysctl, parsing, files
  // ---------------------------------------------------------------------------------------------------------------

  struct Tool {
    started: bool,
    status: Option<i32>,
    timed_out: bool,
    stdout: String,
    stderr: String,
  }

  impl Tool {
    fn ok(&self) -> bool {
      self.started && !self.timed_out && self.status == Some(0)
    }

    fn problem(&self) -> String {
      let first = self.stderr.lines().next().unwrap_or("").trim();
      if !self.started {
        format!("could not start ({first})")
      } else if self.timed_out {
        "did not finish in time and was stopped".to_string()
      } else {
        match self.status {
          Some(c) => format!("exit code {c}{}", if first.is_empty() { String::new() } else { format!(": {first}") }),
          None => "ended by a signal".to_string(),
        }
      }
    }
  }

  /// Runs a system tool by its absolute path and stops it when the time is up. Both pipes are drained on their own
  /// threads, so a chatty tool cannot fill a pipe and stall.
  fn run_tool(path: &str, args: &[&str], limit: Duration) -> Tool {
    assert!(path.starts_with('/'), "tools are run by absolute path only");
    let spawned = Command::new(path).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match spawned {
      Ok(c) => c,
      Err(e) => return Tool { started: false, status: None, timed_out: false, stdout: String::new(), stderr: e.to_string() },
    };
    let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
    let drain_out = std::thread::spawn(move || {
      let mut b = Vec::new();
      if let Some(o) = out.as_mut() {
        let _ = o.read_to_end(&mut b);
      }
      b
    });
    let drain_err = std::thread::spawn(move || {
      let mut b = Vec::new();
      if let Some(e) = err.as_mut() {
        let _ = e.read_to_end(&mut b);
      }
      b
    });
    let deadline = Instant::now() + limit;
    let (status, timed_out) = loop {
      match child.try_wait() {
        Ok(Some(s)) => break (s.code(), false),
        Ok(None) if Instant::now() >= deadline => {
          let _ = child.kill();
          let _ = child.wait();
          break (None, true);
        }
        Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        Err(_) => break (None, false),
      }
    };
    let stdout = String::from_utf8_lossy(&drain_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&drain_err.join().unwrap_or_default()).into_owned();
    Tool { started: true, status, timed_out, stdout, stderr }
  }

  fn sysctl_raw(name: &str) -> Option<Vec<u8>> {
    let c = CString::new(name).ok()?;
    let mut len: libc::size_t = 0;
    // SAFETY: the first call only asks for the size; the second fills a buffer of exactly that size.
    unsafe {
      if libc::sysctlbyname(c.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) != 0 {
        return None;
      }
      let mut buf = vec![0u8; len];
      if libc::sysctlbyname(c.as_ptr(), buf.as_mut_ptr() as *mut c_void, &mut len, std::ptr::null_mut(), 0) != 0 {
        return None;
      }
      buf.truncate(len);
      Some(buf)
    }
  }

  fn sysctl_int(name: &str) -> Option<i64> {
    match sysctl_raw(name)?.as_slice() {
      [a, b, c, d] => Some(i32::from_le_bytes([*a, *b, *c, *d]) as i64),
      [a, b, c, d, e, f, g, h] => Some(i64::from_le_bytes([*a, *b, *c, *d, *e, *f, *g, *h])),
      _ => None,
    }
  }

  fn sysctl_string(name: &str) -> Option<String> {
    let mut b = sysctl_raw(name)?;
    while b.last() == Some(&0) {
      b.pop();
    }
    Some(String::from_utf8_lossy(&b).into_owned())
  }

  fn opt<T: ToString>(v: Option<T>) -> String {
    v.map_or_else(|| "not present".to_string(), |v| v.to_string())
  }

  fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
  }

  /// Every number that follows `"key"` in ioreg's output (`"key" = 12` and `"key"=12` both work).
  fn ioreg_numbers(text: &str, key: &str) -> Vec<f64> {
    let pat = format!("\"{key}\"");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&pat) {
      let after = &rest[i + pat.len()..];
      let num: String = after.trim_start_matches([' ', '=']).chars().take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '.').collect();
      if let Ok(v) = num.parse::<f64>() {
        out.push(v);
      }
      rest = after;
    }
    out
  }

  /// Every quoted string that follows `"key"` in ioreg's output.
  fn ioreg_strings(text: &str, key: &str) -> Vec<String> {
    let pat = format!("\"{key}\"");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&pat) {
      let after = &rest[i + pat.len()..];
      if let Some(v) = after.trim_start_matches([' ', '=']).strip_prefix('"') {
        if let Some(end) = v.find('"') {
          out.push(v[..end].to_string());
        }
      }
      rest = after;
    }
    out
  }

  fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
  }

  /// A failed system call, kept as the number so tests can compare it and printed in words.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  struct Errno(i32);

  impl std::fmt::Display for Errno {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      write!(f, "{}", std::io::Error::from_raw_os_error(self.0))
    }
  }

  impl std::error::Error for Errno {}

  /// A file descriptor, closed when dropped.
  struct Fd(c_int);

  impl Drop for Fd {
    fn drop(&mut self) {
      // SAFETY: this value owns the descriptor.
      unsafe { libc::close(self.0) };
    }
  }

  fn cpath(p: &Path) -> CString {
    CString::new(p.as_os_str().as_bytes()).unwrap_or_default()
  }

  fn cname(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
  }

  const DIR: c_int = libc::O_RDONLY | libc::O_DIRECTORY;

  fn open_path(path: &Path, flags: c_int) -> Result<Fd, Errno> {
    let p = cpath(path);
    // SAFETY: `p` is a valid C string; the mode is only read when a file is created.
    let fd = unsafe { libc::open(p.as_ptr(), flags | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
    if fd < 0 { Err(Errno(errno())) } else { Ok(Fd(fd)) }
  }

  fn open_at(dir: c_int, name: &str, flags: c_int) -> Result<Fd, Errno> {
    let n = cname(name);
    // SAFETY: as `open_path`.
    let fd = unsafe { libc::openat(dir, n.as_ptr(), flags | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
    if fd < 0 { Err(Errno(errno())) } else { Ok(Fd(fd)) }
  }

  fn unlink_at(dir: &Fd, name: &str) -> Result<(), Errno> {
    let n = cname(name);
    // SAFETY: `n` is a valid C string and `dir` is open.
    if unsafe { libc::unlinkat(dir.0, n.as_ptr(), 0) } == 0 { Ok(()) } else { Err(Errno(errno())) }
  }

  fn stat_fd(fd: &Fd) -> Result<libc::stat, Errno> {
    // SAFETY: an all-zero `stat` is a valid value to be filled in.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fd` is open and `st` is a valid buffer.
    if unsafe { libc::fstat(fd.0, &mut st) } == 0 { Ok(st) } else { Err(Errno(errno())) }
  }

  fn stat_at_nofollow(dir: &Fd, name: &str) -> Result<libc::stat, Errno> {
    let n = cname(name);
    // SAFETY: as `stat_fd`.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(dir.0, n.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0 { Ok(st) } else { Err(Errno(errno())) }
  }

  /// The path the kernel says an open descriptor is at (`F_GETPATH`, or `F_GETPATH_NOFIRMLINK`).
  fn fd_path(fd: &Fd, cmd: c_int) -> Result<String, Errno> {
    let mut buf = [0 as c_char; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH needs a buffer of at least PATH_MAX bytes, which this is.
    if unsafe { libc::fcntl(fd.0, cmd, buf.as_mut_ptr()) } != 0 {
      return Err(Errno(errno()));
    }
    // SAFETY: the kernel wrote a NUL-terminated string.
    Ok(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
  }

  fn kind_of(mode: u16) -> &'static str {
    match mode & libc::S_IFMT {
      libc::S_IFREG => "file",
      libc::S_IFDIR => "directory",
      libc::S_IFLNK => "symlink",
      _ => "other",
    }
  }

  fn mkdir(parent: &Path, name: &str) -> std::io::Result<PathBuf> {
    let p = parent.join(name);
    fs::create_dir_all(&p)?;
    Ok(p)
  }

  fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
  }

  // ---------------------------------------------------------------------------------------------------------------
  // The machine
  // ---------------------------------------------------------------------------------------------------------------

  struct Machine {
    arch: String,
    apple_silicon: bool,
    translated: Option<i64>,
    model: String,
    os: String,
    vm: Option<i64>,
    cpu: String,
    cores: Option<i64>,
    mem_bytes: Option<i64>,
    perf_levels: Option<i64>,
    p0: Option<i64>,
    p1: Option<i64>,
    freq_hz: Option<i64>,
    github: bool,
  }

  fn machine_info() -> Machine {
    // SAFETY: an all-zero `utsname` is valid, and `uname` fills it with NUL-terminated strings.
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    let arch = if unsafe { libc::uname(&mut u) } == 0 { unsafe { CStr::from_ptr(u.machine.as_ptr()) }.to_string_lossy().into_owned() } else { "unknown".into() };
    Machine {
      arch,
      apple_silicon: sysctl_int("hw.optional.arm64") == Some(1),
      translated: sysctl_int("sysctl.proc_translated"),
      model: sysctl_string("hw.model").unwrap_or_else(|| "unknown model".into()),
      os: sysctl_string("kern.osproductversion").unwrap_or_else(|| "unknown".into()),
      vm: sysctl_int("kern.hv_vmm_present"),
      cpu: sysctl_string("machdep.cpu.brand_string").unwrap_or_else(|| "unknown".into()),
      cores: sysctl_int("hw.ncpu"),
      mem_bytes: sysctl_int("hw.memsize"),
      perf_levels: sysctl_int("hw.nperflevels"),
      p0: sysctl_int("hw.perflevel0.logicalcpu"),
      p1: sysctl_int("hw.perflevel1.logicalcpu"),
      freq_hz: sysctl_int("hw.cpufrequency"),
      github: std::env::var_os("GITHUB_ACTIONS").is_some(),
    }
  }

  impl Machine {
    /// One line that tells a CI virtual machine from a real Mac, put on every check.
    fn kind(&self) -> String {
      let hw = if self.apple_silicon { "Apple Silicon" } else { "Intel" };
      let rosetta = if self.translated == Some(1) { " (running under Rosetta)" } else { "" };
      let real = match self.vm {
        Some(1) => "virtual machine",
        Some(_) => "real Mac",
        None => "unknown whether virtual",
      };
      format!("{hw}{rosetta}, {real}, {}, macOS {}{}", self.model, self.os, if self.github { ", GitHub Actions" } else { "" })
    }

    fn evidence(&self) -> String {
      format!(
        "hardware: {} (uname -m says {}, this binary is built for {})\nRosetta (sysctl.proc_translated): {}\nmodel (hw.model): {}\nmacOS: {}\ncpu: {}\nlogical cpus (hw.ncpu): {}\nmemory (hw.memsize): {}\nperformance levels (hw.nperflevels): {}; level 0 logical cpus: {}; level 1 logical cpus: {}\nhw.cpufrequency (Intel only): {}\nvirtual machine (kern.hv_vmm_present): {}\nGitHub Actions: {}",
        if self.apple_silicon { "Apple Silicon" } else { "Intel" },
        self.arch,
        std::env::consts::ARCH,
        opt(self.translated),
        self.model,
        self.os,
        self.cpu,
        opt(self.cores),
        self.mem_bytes.map_or("not present".into(), |b| gb(b as u64)),
        opt(self.perf_levels),
        opt(self.p0),
        opt(self.p1),
        self.freq_hz.map_or("not present".into(), |h| format!("{:.2} GHz", h as f64 / 1e9)),
        opt(self.vm),
        yes(self.github),
      )
    }
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Sensors
  // ---------------------------------------------------------------------------------------------------------------

  #[link(name = "objc")]
  unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn objc_msgSend();
  }

  // Foundation has to be linked for NSProcessInfo to exist. Nothing is called from it directly.
  #[link(name = "Foundation", kind = "framework")]
  unsafe extern "C" {}

  #[link(name = "IOKit", kind = "framework")]
  unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
    fn IOServiceOpen(service: u32, owning_task: u32, kind: u32, connect: *mut u32) -> i32;
    fn IOServiceClose(connect: u32) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOConnectCallStructMethod(connect: u32, selector: u32, input: *const c_void, input_size: usize, output: *mut c_void, output_size: *mut usize) -> i32;
  }

  fn thermal_state() -> Res {
    type SendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type SendInt = unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize;
    // SAFETY: objc_msgSend is declared without arguments and cast to the exact signature of each call, which is how it
    // must be called (it is not a variadic function).
    unsafe {
      let send_id: SendId = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
      let send_int: SendInt = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
      let class = objc_getClass(c"NSProcessInfo".as_ptr());
      if class.is_null() {
        return Ok((Fails, "the NSProcessInfo class was not found".into()));
      }
      let info = send_id(class, sel_registerName(c"processInfo".as_ptr()));
      if info.is_null() {
        return Ok((Fails, "[NSProcessInfo processInfo] gave nothing".into()));
      }
      let state = send_int(info, sel_registerName(c"thermalState".as_ptr()));
      let word = match state {
        0 => "nominal",
        1 => "fair",
        2 => "serious",
        3 => "critical",
        _ => "unknown value",
      };
      Ok((Works, format!("NSProcessInfo.thermalState = {state} ({word}). Four coarse levels and no temperature, but it needs no permission.")))
    }
  }

  fn xcpm_levels() -> Res {
    let names = ["machdep.xcpm.cpu_thermal_level", "machdep.xcpm.gpu_thermal_level", "machdep.xcpm.io_thermal_level"];
    let found: Vec<String> = names.iter().filter_map(|n| sysctl_int(n).map(|v| format!("{n} = {v}"))).collect();
    if found.is_empty() {
      Ok((NotPresent, "none of the machdep.xcpm.*_thermal_level values exist (expected on Apple Silicon; Intel Macs have them)".into()))
    } else {
      Ok((Works, found.join("\n")))
    }
  }

  // The SMC is only ever read here. Commands 9 (key info), 5 (read key) and 8 (key name at an index) are reads. The write
  // command is deliberately not defined anywhere in this file.
  #[repr(C)]
  #[derive(Clone, Copy, Default)]
  struct SmcVers {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
  }

  #[repr(C)]
  #[derive(Clone, Copy, Default)]
  struct SmcPLimit {
    version: u16,
    length: u16,
    cpu: u32,
    gpu: u32,
    mem: u32,
  }

  #[repr(C)]
  #[derive(Clone, Copy, Default)]
  struct SmcKeyInfo {
    data_size: u32,
    data_type: u32,
    data_attributes: u8,
  }

  #[repr(C)]
  #[derive(Clone, Copy, Default)]
  struct SmcKeyData {
    key: u32,
    vers: SmcVers,
    p_limit: SmcPLimit,
    key_info: SmcKeyInfo,
    result: u8,
    status: u8,
    data8: u8,
    data32: u32,
    bytes: [u8; 32],
  }

  // The kernel checks this size exactly, so a wrong layout must not compile.
  const _: () = assert!(std::mem::size_of::<SmcKeyData>() == 80);

  const SMC_HANDLE_YPC_EVENT: u32 = 2;
  const SMC_READ_KEY: u8 = 5;
  const SMC_READ_INDEX: u8 = 8;
  const SMC_GET_KEY_INFO: u8 = 9;

  enum SmcErr {
    /// The SMC answered, and has no such key (result 0x84).
    Missing,
    Failed(String),
  }

  impl std::fmt::Display for SmcErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      match self {
        SmcErr::Missing => write!(f, "no such key"),
        SmcErr::Failed(s) => write!(f, "{s}"),
      }
    }
  }

  struct SmcValue {
    ty: String,
    size: usize,
    bytes: Vec<u8>,
  }

  struct Smc {
    conn: u32,
  }

  impl Drop for Smc {
    fn drop(&mut self) {
      // SAFETY: this value owns the connection.
      unsafe { IOServiceClose(self.conn) };
    }
  }

  fn fourcc(s: &str) -> u32 {
    let mut b = [b' '; 4];
    for (i, c) in s.bytes().take(4).enumerate() {
      b[i] = c;
    }
    u32::from_be_bytes(b)
  }

  fn fourcc_text(v: u32) -> String {
    String::from_utf8_lossy(&v.to_be_bytes()).into_owned()
  }

  impl Smc {
    /// `Err(None)` means there is no AppleSMC service at all.
    fn open() -> Result<Smc, Option<String>> {
      // SAFETY: plain IOKit calls. The matching dictionary is consumed by the lookup, and the service object is released
      // once it has been opened.
      unsafe {
        let matching = IOServiceMatching(c"AppleSMC".as_ptr());
        if matching.is_null() {
          return Err(Some("IOServiceMatching gave nothing".into()));
        }
        let service = IOServiceGetMatchingService(0, matching);
        if service == 0 {
          return Err(None);
        }
        let mut conn = 0u32;
        #[allow(deprecated)] // libc points to the mach2 crate for this; one call does not need another dependency.
        let task = libc::mach_task_self();
        let kr = IOServiceOpen(service, task, 0, &mut conn);
        IOObjectRelease(service);
        if kr != 0 {
          return Err(Some(format!("IOServiceOpen failed (kern_return 0x{:x})", kr as u32)));
        }
        Ok(Smc { conn })
      }
    }

    fn call(&self, input: &SmcKeyData) -> Result<SmcKeyData, SmcErr> {
      let mut out = SmcKeyData::default();
      let mut size = std::mem::size_of::<SmcKeyData>();
      // SAFETY: both buffers are the 80-byte struct the SMC expects, and `size` says so.
      let kr = unsafe { IOConnectCallStructMethod(self.conn, SMC_HANDLE_YPC_EVENT, input as *const _ as *const c_void, size, &mut out as *mut _ as *mut c_void, &mut size) };
      if kr != 0 {
        return Err(SmcErr::Failed(format!("IOConnectCallStructMethod failed (kern_return 0x{:x})", kr as u32)));
      }
      match out.result {
        0 => Ok(out),
        0x84 => Err(SmcErr::Missing),
        other => Err(SmcErr::Failed(format!("the SMC answered with result 0x{other:02x}"))),
      }
    }

    fn read(&self, key: &str) -> Result<SmcValue, SmcErr> {
      let mut ask = SmcKeyData { key: fourcc(key), data8: SMC_GET_KEY_INFO, ..Default::default() };
      let info = self.call(&ask)?.key_info;
      ask.key_info.data_size = info.data_size;
      ask.data8 = SMC_READ_KEY;
      let got = self.call(&ask)?;
      let size = (info.data_size as usize).min(32);
      Ok(SmcValue { ty: fourcc_text(info.data_type), size, bytes: got.bytes[..size].to_vec() })
    }

    fn key_at(&self, index: u32) -> Result<String, SmcErr> {
      let ask = SmcKeyData { data8: SMC_READ_INDEX, data32: index, ..Default::default() };
      Ok(fourcc_text(self.call(&ask)?.key))
    }
  }

  fn smc_decode(ty: &str, b: &[u8]) -> Option<f64> {
    match ty {
      "sp78" if b.len() >= 2 => Some(i16::from_be_bytes([b[0], b[1]]) as f64 / 256.0),
      "fpe2" if b.len() >= 2 => Some(u16::from_be_bytes([b[0], b[1]]) as f64 / 4.0),
      "flt " if b.len() >= 4 => Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64),
      "ui8 " if !b.is_empty() => Some(b[0] as f64),
      "ui16" if b.len() >= 2 => Some(u16::from_be_bytes([b[0], b[1]]) as f64),
      "ui32" if b.len() >= 4 => Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64),
      _ => None,
    }
  }

  fn smc_check() -> Res {
    let smc = match Smc::open() {
      Ok(s) => s,
      Err(None) => return Ok((NotPresent, "IOServiceGetMatchingService found no AppleSMC service".into())),
      Err(Some(why)) => return Ok((Fails, why)),
    };
    let mut lines = vec!["opened AppleSMC without root".to_string()];
    let count = smc.read("#KEY").ok().and_then(|v| smc_decode(&v.ty, &v.bytes));
    lines.push(format!("#KEY (number of keys): {}", count.map_or("could not be read".to_string(), |c| format!("{c:.0}"))));

    let mut plausible: BTreeMap<String, f64> = BTreeMap::new();
    for key in ["TC0P", "TC0D", "TC0E", "Tp01", "Tp05", "Tp09", "Te05", "Tg05"] {
      match smc.read(key) {
        Ok(v) => match smc_decode(&v.ty, &v.bytes) {
          Some(t) => {
            lines.push(format!("{key}: type {} size {} = {t:.1}", v.ty.trim_end(), v.size));
            if t > 0.0 && t < 130.0 {
              plausible.insert(key.to_string(), t);
            }
          }
          None => lines.push(format!("{key}: type {} size {} (not decoded)", v.ty.trim_end(), v.size)),
        },
        Err(SmcErr::Missing) => lines.push(format!("{key}: not on this machine")),
        Err(e) => lines.push(format!("{key}: {e}")),
      }
    }
    for key in ["FNum", "F0Ac", "F0Mn", "F0Mx"] {
      match smc.read(key) {
        Ok(v) => lines.push(format!("{key}: type {} size {} = {}", v.ty.trim_end(), v.size, smc_decode(&v.ty, &v.bytes).map_or("not decoded".to_string(), |x| format!("{x:.1}")))),
        Err(SmcErr::Missing) => lines.push(format!("{key}: not on this machine")),
        Err(e) => lines.push(format!("{key}: {e}")),
      }
    }

    // Which temperature keys does this model have? Ask for the name at each index (a read) and keep the ones starting with T.
    if let Some(total) = count.map(|c| c as u32).filter(|c| *c > 0 && *c < 20_000) {
      let deadline = Instant::now() + Duration::from_secs(8);
      let mut t_keys = Vec::new();
      let mut listed = 0u32;
      for i in 0..total {
        if Instant::now() > deadline {
          break;
        }
        match smc.key_at(i) {
          Ok(k) => {
            listed += 1;
            if k.starts_with('T') {
              t_keys.push(k);
            }
          }
          Err(_) => break,
        }
      }
      let mut read_ok = 0;
      for k in t_keys.iter().take(300) {
        if let Ok(v) = smc.read(k) {
          if let Some(t) = smc_decode(&v.ty, &v.bytes).filter(|t| *t > 0.0 && *t < 130.0 && (v.ty == "sp78" || v.ty == "flt ")) {
            read_ok += 1;
            plausible.entry(k.clone()).or_insert(t);
          }
        }
      }
      let sample: Vec<String> = plausible.iter().take(12).map(|(k, t)| format!("{k}={t:.0}")).collect();
      lines.push(format!("listed {listed} of {total} key names; {} start with T; {read_ok} of those read as a plausible temperature (sample: {})", t_keys.len(), sample.join(" ")));
    }

    let verdict = if !plausible.is_empty() {
      Works
    } else if count.is_some() {
      Partial
    } else {
      Fails
    };
    lines.push(format!("plausible temperatures found: {}", plausible.len()));
    Ok((verdict, lines.join("\n")))
  }

  fn sensors(r: &mut Report) {
    r.check("sensors: NSProcessInfo.thermalState", thermal_state);
    r.check("sensors: machdep.xcpm thermal levels", xcpm_levels);
    r.check("sensors: SMC temperatures and fans (read only)", smc_check);
  }

  // ---------------------------------------------------------------------------------------------------------------
  // GPU
  // ---------------------------------------------------------------------------------------------------------------

  fn gpu_ioreg() -> Res {
    let t = run_tool("/usr/sbin/ioreg", &["-r", "-c", "IOAccelerator", "-d", "1"], Duration::from_secs(20));
    if !t.ok() {
      return Ok((Fails, format!("ioreg {}", t.problem())));
    }
    let models: BTreeSet<String> = ioreg_strings(&t.stdout, "model").into_iter().collect();
    let mut lines = vec![format!("accelerators listed: {}", t.stdout.matches("<class ").count()), format!("models: {}", if models.is_empty() { "none named".to_string() } else { models.into_iter().collect::<Vec<_>>().join(", ") })];
    let mut any_util = false;
    for key in ["Device Utilization %", "Renderer Utilization %", "Tiler Utilization %", "gpu-core-count"] {
      let v = ioreg_numbers(&t.stdout, key);
      any_util |= key.ends_with('%') && !v.is_empty();
      lines.push(format!("{key}: {}", if v.is_empty() { "not reported".to_string() } else { v.iter().map(|x| format!("{x:.0}")).collect::<Vec<_>>().join(", ") }));
    }
    if t.stdout.matches("<class ").count() == 0 {
      return Ok((NotPresent, "ioreg found no IOAccelerator".into()));
    }
    Ok((if any_util { Works } else { Partial }, lines.join("\n")))
  }

  fn gpu_profiler() -> Res {
    let t = run_tool("/usr/sbin/system_profiler", &["SPDisplaysDataType", "-json"], Duration::from_secs(60));
    if !t.ok() {
      return Ok((Fails, format!("system_profiler {}", t.problem())));
    }
    let v: Value = serde_json::from_str(&t.stdout)?;
    let cards = v["SPDisplaysDataType"].as_array().cloned().unwrap_or_default();
    if cards.is_empty() {
      return Ok((NotPresent, "system_profiler listed no graphics device".into()));
    }
    let lines: Vec<String> = cards
      .iter()
      .map(|c| {
        let get = |k: &str| c[k].as_str().unwrap_or("not given").to_string();
        format!("{} | vendor {} | cores {} | bus {} | metal {}", get("sppci_model"), get("spdisplays_vendor"), get("sppci_cores"), get("sppci_bus"), c["spdisplays_mtlgpufamilysupport"].as_str().or(c["spdisplays_metal"].as_str()).unwrap_or("not given"))
      })
      .collect();
    Ok((Works, format!("{} graphics device(s)\n{}", cards.len(), lines.join("\n"))))
  }

  fn gpu(r: &mut Report) {
    r.check("gpu: ioreg IOAccelerator utilization", gpu_ioreg);
    r.check("gpu: system_profiler SPDisplaysDataType", gpu_profiler);
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Memory
  // ---------------------------------------------------------------------------------------------------------------

  fn mem_sysinfo() -> Res {
    let mut sys = System::new();
    sys.refresh_memory();
    let (total, avail, used) = (sys.total_memory(), sys.available_memory(), sys.used_memory());
    let ok = total > 0 && avail > 0 && used > 0;
    Ok((if ok { Works } else { Partial }, format!("total {} | available {} | used {} | swap total {} used {}", gb(total), gb(avail), gb(used), gb(sys.total_swap()), gb(sys.used_swap()))))
  }

  fn mem_vm_stat() -> Res {
    let t = run_tool("/usr/bin/vm_stat", &[], Duration::from_secs(10));
    if !t.ok() {
      return Ok((Fails, format!("vm_stat {}", t.problem())));
    }
    let page: u64 = t.stdout.lines().next().and_then(|l| l.split("page size of ").nth(1)).and_then(|s| s.split_whitespace().next()).and_then(|n| n.parse().ok()).unwrap_or(4096);
    let mut found = Vec::new();
    for key in ["Pages free", "Pages active", "Pages inactive", "Pages wired down", "Pages occupied by compressor", "Pages stored in compressor", "Swapins", "Swapouts"] {
      let n = t.stdout.lines().find(|l| l.starts_with(key)).and_then(|l| l.split(':').nth(1)).map(|v| v.trim().trim_end_matches('.')).and_then(|v| v.parse::<u64>().ok());
      found.push(match n {
        Some(n) if key.starts_with("Pages") => format!("{key}: {n} ({})", gb(n * page)),
        Some(n) => format!("{key}: {n}"),
        None => format!("{key}: not given"),
      });
    }
    let all = found.iter().all(|f| !f.ends_with("not given"));
    Ok((if all { Works } else { Partial }, format!("page size {page} bytes\n{}", found.join("\n"))))
  }

  fn mem_swap() -> Res {
    let Some(b) = sysctl_raw("vm.swapusage") else { return Ok((NotPresent, "vm.swapusage is not there".into())) };
    if b.len() < 24 {
      return Ok((Partial, format!("vm.swapusage gave {} bytes, expected at least 24", b.len())));
    }
    let n = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or([0; 8]));
    Ok((Works, format!("total {} | available {} | used {}", gb(n(0)), gb(n(8)), gb(n(16)))))
  }

  fn mem_pressure() -> Res {
    let level = sysctl_int("kern.memorystatus_vm_pressure_level");
    let free_pct = sysctl_int("kern.memorystatus_level");
    let word = match level {
      Some(1) => "normal",
      Some(2) => "warning",
      Some(4) => "critical",
      Some(_) => "unexpected value",
      None => "not present",
    };
    let verdict = if level.is_some() { Works } else if free_pct.is_some() { Partial } else { NotPresent };
    Ok((verdict, format!("kern.memorystatus_vm_pressure_level = {} ({word})\nkern.memorystatus_level (percent of memory the system counts as free) = {}", opt(level), opt(free_pct))))
  }

  fn mem_compressor() -> Res {
    let t = run_tool("/usr/sbin/sysctl", &["-a"], Duration::from_secs(20));
    if !t.ok() {
      return Ok((Fails, format!("sysctl -a {}", t.problem())));
    }
    let lines: Vec<&str> = t.stdout.lines().filter(|l| l.to_lowercase().contains("compress")).filter(|l| l.split(':').nth(1).is_some_and(|v| v.trim().parse::<f64>().is_ok())).take(30).collect();
    if lines.is_empty() {
      return Ok((NotPresent, "no numeric sysctl with 'compress' in its name".into()));
    }
    Ok((Works, format!("{} numeric compressor values, for example:\n{}", lines.len(), lines.join("\n"))))
  }

  fn memory(r: &mut Report) {
    r.check("memory: sysinfo totals", mem_sysinfo);
    r.check("memory: vm_stat", mem_vm_stat);
    r.check("memory: vm.swapusage", mem_swap);
    r.check("memory: pressure level", mem_pressure);
    r.check("memory: page compressor values", mem_compressor);
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Battery
  // ---------------------------------------------------------------------------------------------------------------

  fn battery_pmset() -> Res {
    let t = run_tool("/usr/bin/pmset", &["-g", "batt"], Duration::from_secs(10));
    if !t.ok() {
      return Ok((Fails, format!("pmset {}", t.problem())));
    }
    let source = t.stdout.lines().next().and_then(|l| l.split('\'').nth(1)).unwrap_or("not given");
    let Some(line) = t.stdout.lines().find(|l| l.contains("InternalBattery")) else {
      return Ok((NotPresent, format!("pmset lists no internal battery (power source: {source})")));
    };
    let detail = line.split('\t').nth(1).unwrap_or("").trim();
    Ok((Works, format!("power source: {source}\ninternal battery: {detail}")))
  }

  fn battery_ioreg() -> Res {
    let t = run_tool("/usr/sbin/ioreg", &["-r", "-n", "AppleSmartBattery"], Duration::from_secs(10));
    if !t.ok() {
      return Ok((Fails, format!("ioreg {}", t.problem())));
    }
    if !t.stdout.contains("AppleSmartBattery") {
      return Ok((NotPresent, "ioreg found no AppleSmartBattery".into()));
    }
    let first = |k: &str| ioreg_numbers(&t.stdout, k).first().copied();
    let show = |k: &str| format!("{k}: {}", first(k).map_or("not given".to_string(), |v| format!("{v:.0}")));
    let temp = first("Temperature").map_or("not given".to_string(), |v| format!("{v:.0} ({:.1} C if hundredths)", v / 100.0));
    let core = first("CycleCount").is_some() && first("DesignCapacity").is_some();
    let lines = [show("CycleCount"), show("DesignCapacity"), show("MaxCapacity"), show("AppleRawMaxCapacity"), show("NominalChargeCapacity"), show("CurrentCapacity"), format!("Temperature: {temp}")];
    Ok((if core { Works } else { Partial }, lines.join("\n")))
  }

  fn battery(r: &mut Report) {
    r.check("battery: pmset -g batt", battery_pmset);
    r.check("battery: ioreg AppleSmartBattery", battery_ioreg);
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Processes
  // ---------------------------------------------------------------------------------------------------------------

  #[derive(Default)]
  struct Tally {
    total: usize,
    name: usize,
    exe: usize,
    cmd: usize,
    uid: usize,
  }

  impl Tally {
    fn text(&self) -> String {
      format!("{} processes: name {}, exe {}, cmd {}, uid {}", self.total, self.name, self.exe, self.cmd, self.uid)
    }
  }

  fn refreshed_processes() -> System {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always).with_user(UpdateKind::Always));
    sys
  }

  fn processes_check() -> Res {
    let sys = refreshed_processes();
    let me = unsafe { libc::getuid() };
    let (mut all, mut root, mut own, mut other, mut unknown) = (Tally::default(), Tally::default(), Tally::default(), Tally::default(), Tally::default());
    for p in sys.processes().values() {
      let uid = p.user_id().map(|u| **u);
      let group = match uid {
        Some(0) => &mut root,
        Some(u) if u == me => &mut own,
        Some(_) => &mut other,
        None => &mut unknown,
      };
      for t in [&mut all, group] {
        t.total += 1;
        t.name += usize::from(!p.name().is_empty());
        t.exe += usize::from(p.exe().is_some());
        t.cmd += usize::from(!p.cmd().is_empty());
        t.uid += usize::from(uid.is_some());
      }
    }
    let mut lines = vec![format!("all: {}", all.text()), format!("root-owned: {}", root.text()), format!("owned by this user: {}", own.text()), format!("owned by other users: {}", other.text()), format!("uid unreadable: {}", unknown.total)];
    for want in ["launchd", "WindowServer", "loginwindow", "Finder", "Dock"] {
      let found = sys.processes().values().find(|p| p.name() == want);
      lines.push(match found {
        Some(p) => format!("{want}: present (pid {}, uid {}, exe readable: {})", p.pid(), opt(p.user_id().map(|u| **u)), yes(p.exe().is_some())),
        None => format!("{want}: not running"),
      });
    }
    let pid1 = sys.process(sysinfo::Pid::from_u32(1)).map(|p| p.name().to_string_lossy().into_owned());
    lines.push(format!("pid 1 is {}", pid1.unwrap_or_else(|| "not visible".into())));
    lines.push("classify() needs a Mac rule for root-owned processes: the uid is what it can use if it is readable for all of them.".into());
    let verdict = if root.total == 0 || root.uid < root.total || all.name < all.total {
      Fails
    } else if root.exe < root.total || root.cmd < root.total {
      Partial
    } else {
      Works
    };
    Ok((verdict, lines.join("\n")))
  }

  fn processes(r: &mut Report) {
    r.check("processes: sysinfo name, exe, cmd and uid", processes_check);
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Safe deletes (the most important check)
  // ---------------------------------------------------------------------------------------------------------------

  /// Opens `rel` one component at a time from a trusted directory, refusing a link anywhere on the way and any `..`.
  /// This is the shape `really_at` and `delete_exact` will take on the Mac.
  fn walk_open(anchor: &Fd, rel: &str) -> Result<Fd, String> {
    if rel.is_empty() || rel.starts_with('/') {
      return Err("the path must be relative and not empty".into());
    }
    let parts: Vec<&str> = rel.split('/').collect();
    let mut held: Option<Fd> = None;
    for (i, part) in parts.iter().enumerate() {
      if part.is_empty() || *part == "." || *part == ".." {
        return Err(format!("refused the component {part:?}"));
      }
      let flags = libc::O_RDONLY | libc::O_NOFOLLOW | if i + 1 < parts.len() { libc::O_DIRECTORY } else { 0 };
      let from = held.as_ref().map_or(anchor.0, |f| f.0);
      held = Some(open_at(from, part, flags).map_err(|e| format!("{part:?}: {e}"))?);
    }
    held.ok_or_else(|| "nothing was opened".to_string())
  }

  fn safe_a_nofollow(root: &Path) -> Res {
    let d = mkdir(root, "a")?;
    fs::write(d.join("real.txt"), "real")?;
    symlink("real.txt", d.join("link.txt"))?;
    let real = open_path(&d.join("real.txt"), libc::O_RDONLY | libc::O_NOFOLLOW);
    let link = open_path(&d.join("link.txt"), libc::O_RDONLY | libc::O_NOFOLLOW);
    let followed = open_path(&d.join("link.txt"), libc::O_RDONLY);
    let as_link = open_path(&d.join("link.txt"), libc::O_RDONLY | libc::O_SYMLINK);
    let as_link_kind = as_link.as_ref().ok().and_then(|f| stat_fd(f).ok()).map(|s| kind_of(s.st_mode));
    let refused = link.is_err();
    let verdict = if real.is_ok() && refused && followed.is_ok() {
      if as_link_kind == Some("symlink") { Works } else { Partial }
    } else {
      Fails
    };
    Ok((
      verdict,
      format!(
        "real file with O_NOFOLLOW: {}\nsymlink with O_NOFOLLOW: {} (refused: {})\nsymlink with plain open (control): {}\nsymlink with O_SYMLINK opens the link itself: {}",
        if real.is_ok() { "opened" } else { "failed" },
        link.as_ref().err().map_or("opened".to_string(), |e| e.to_string()),
        yes(refused),
        if followed.is_ok() { "followed the link" } else { "failed" },
        as_link_kind.map_or("failed".to_string(), |k| format!("yes, fstat says {k}")),
      ),
    ))
  }

  fn safe_b_walk(root: &Path) -> Res {
    let base = mkdir(root, "b")?;
    let trusted = mkdir(&base, "trusted")?;
    let outside = mkdir(&base, "outside")?;
    fs::create_dir(trusted.join("sub"))?;
    fs::write(trusted.join("sub").join("file.txt"), "inside")?;
    fs::write(outside.join("secret.txt"), "outside")?;
    symlink(&outside, trusted.join("linkdir"))?;
    symlink("file.txt", trusted.join("sub").join("flink"))?;
    let anchor = open_path(&trusted, DIR)?;

    let good = walk_open(&anchor, "sub/file.txt");
    let good_ino = good.as_ref().ok().and_then(|f| stat_fd(f).ok()).map(|s| s.st_ino);
    let real_ino = fs::metadata(trusted.join("sub").join("file.txt"))?.ino();
    let via_dir = walk_open(&anchor, "linkdir/secret.txt");
    let via_file = walk_open(&anchor, "sub/flink");
    let dotdot = walk_open(&anchor, "sub/../sub/file.txt");
    // Controls: what the system does without those checks.
    let raw_up = open_at(anchor.0, "..", DIR);
    let raw_follow = open_at(anchor.0, "linkdir/secret.txt", libc::O_RDONLY);
    // macOS 11 and later can refuse a link anywhere in a relative path in one call.
    let any_good = open_at(anchor.0, "sub/file.txt", libc::O_RDONLY | libc::O_NOFOLLOW_ANY);
    let any_bad = open_at(anchor.0, "linkdir/secret.txt", libc::O_RDONLY | libc::O_NOFOLLOW_ANY);

    let pass = good_ino == Some(real_ino) && via_dir.is_err() && via_file.is_err() && dotdot.is_err();
    let text = |r: &Result<Fd, String>| match r {
      Ok(_) => "opened".to_string(),
      Err(e) => format!("refused ({e})"),
    };
    Ok((
      if pass { Works } else { Fails },
      format!(
        "sub/file.txt: {} (same file as the real one: {})\nlinkdir/secret.txt, a symlink in the middle: {}\nsub/flink, a symlink at the end: {}\nsub/../sub/file.txt, a '..': {}\ncontrols without those checks: openat(fd, \"..\") {}; openat of a path through the symlink {}\nO_NOFOLLOW_ANY (macOS 11+): plain relative path {}; path through a symlink {}",
        text(&good),
        yes(good_ino == Some(real_ino)),
        text(&via_dir),
        text(&via_file),
        text(&dotdot),
        if raw_up.is_ok() { "climbs out of the trusted folder" } else { "refused" },
        if raw_follow.is_ok() { "reads the file outside" } else { "refused" },
        if any_good.is_ok() { "opened" } else { "refused" },
        any_bad.as_ref().err().map_or("opened (not enforced)".to_string(), |e| format!("refused ({e})")),
      ),
    ))
  }

  fn safe_c_parent_swap(root: &Path) -> Res {
    let base = mkdir(root, "c")?;
    let parent = mkdir(&base, "parent")?;
    let victim = mkdir(&base, "victim")?;
    fs::write(parent.join("target.txt"), "mine")?;
    fs::write(victim.join("target.txt"), "victim-file")?;
    let held = open_path(&parent, DIR)?;
    // The attacker swaps the parent's path for a link to the victim folder after the directory was opened.
    let moved = base.join("parent_moved");
    fs::rename(&parent, &moved)?;
    symlink(&victim, &parent)?;
    let path_now_reaches_victim = fs::read_to_string(parent.join("target.txt")).is_ok_and(|s| s == "victim-file");
    unlink_at(&held, "target.txt")?;
    let mine_gone = !moved.join("target.txt").exists();
    let victim_kept = fs::read_to_string(victim.join("target.txt")).is_ok_and(|s| s == "victim-file");

    // Control: a delete by path, in the same situation, does go to the victim.
    fs::write(moved.join("control.txt"), "mine")?;
    fs::write(victim.join("control.txt"), "victim-control")?;
    fs::remove_file(parent.join("control.txt"))?;
    let control_hit_victim = !victim.join("control.txt").exists();

    Ok((
      if mine_gone && victim_kept { Works } else { Fails },
      format!(
        "after the swap, the old path leads to the victim folder: {}\nunlinkat on the held directory removed the original file: {}\nthe victim's file survived: {}\ncontrol, deleting by path in the same situation: {}",
        yes(path_now_reaches_victim),
        yes(mine_gone),
        yes(victim_kept),
        if control_hit_victim { "it deleted the victim's file, so the attack is real and the held directory is what stops it" } else { "it did not reach the victim (the attack did not reproduce)" },
      ),
    ))
  }

  fn safe_c2_file_swap(root: &Path) -> Res {
    let d = mkdir(root, "c2")?;
    fs::write(d.join("t.txt"), "original")?;
    fs::write(d.join("v.txt"), "victim")?;
    let dir = open_path(&d, DIR)?;
    let held = open_at(dir.0, "t.txt", libc::O_RDONLY | libc::O_NOFOLLOW)?;
    let before = stat_fd(&held)?;
    // After the check, the file is replaced by a link to another file.
    fs::remove_file(d.join("t.txt"))?;
    symlink(d.join("v.txt"), d.join("t.txt"))?;
    let now = stat_at_nofollow(&dir, "t.txt")?;
    let detected = (now.st_dev, now.st_ino) != (before.st_dev, before.st_ino) || kind_of(now.st_mode) == "symlink";
    unlink_at(&dir, "t.txt")?;
    let victim_kept = fs::read_to_string(d.join("v.txt")).is_ok_and(|s| s == "victim");
    Ok((
      if detected && victim_kept { Works } else { Fails },
      format!(
        "the name was swapped for a symlink after it was checked\nfstatat(AT_SYMLINK_NOFOLLOW) right before unlinkat shows a different object: {} (it says {})\nunlinkat removed only the link, and the file it pointed to is intact: {}\nunlinkat cannot leave the held directory, but it removes whatever has that name at that moment, so delete_exact should compare device and inode with the checked file just before it.",
        yes(detected),
        kind_of(now.st_mode),
        yes(victim_kept),
      ),
    ))
  }

  fn safe_d_getpath(root: &Path) -> Res {
    let d = mkdir(root, "d")?;
    fs::write(d.join("Mixed-Case.txt"), "x")?;
    let exact = open_path(&d.join("Mixed-Case.txt"), libc::O_RDONLY)?;
    let exact_path = fd_path(&exact, libc::F_GETPATH)?;
    let wrong = open_path(&d.join("mixed-case.TXT"), libc::O_RDONLY);
    let wrong_path = wrong.as_ref().ok().map(|f| fd_path(f, libc::F_GETPATH));
    let tmp = open_path(Path::new("/tmp"), DIR)?;
    let tmp_path = fd_path(&tmp, libc::F_GETPATH)?;
    let home = std::env::var("HOME").unwrap_or_default();
    let home_fd = open_path(Path::new(&home), DIR);
    let home_paths = home_fd.as_ref().ok().map(|f| (fd_path(f, libc::F_GETPATH), fd_path(f, libc::F_GETPATH_NOFIRMLINK)));
    let last = |p: &str| p.rsplit('/').next().unwrap_or("").to_string();
    let resolved = exact_path.starts_with("/private/var/") || exact_path.starts_with("/var/");
    let works = tmp_path == "/private/tmp" && resolved;
    let home_text = match home_paths {
      Some((a, b)) => format!("the home folder: F_GETPATH ends in {} and F_GETPATH_NOFIRMLINK {}", a.map_or("an error".to_string(), |p| last(&p)), b.map_or("is an error".to_string(), |p| if p.starts_with("/System/Volumes/Data/") { "starts with /System/Volumes/Data/".to_string() } else { format!("starts with {}", p.split('/').take(2).collect::<Vec<_>>().join("/")) })),
      None => "the home folder could not be opened".to_string(),
    };
    Ok((
      if works { Works } else { Partial },
      format!(
        "the probe folder's temp path (a link into /private/var) comes back resolved: {} ({})\n/tmp comes back as {tmp_path}\nopening the file under a name in the wrong case: {}\n{home_text}\nA check that compares F_GETPATH with the wanted path must compare against the resolved path of the trusted folder, and must not rely on the case the caller typed.",
        yes(resolved),
        exact_path.split('/').take(4).collect::<Vec<_>>().join("/"),
        match (&wrong, wrong_path) {
          (Err(e), _) => format!("refused ({e}), so this volume is case-sensitive"),
          (Ok(_), Some(Ok(p))) => format!("opened the same file, and F_GETPATH ends in {:?} (the name on disk is \"Mixed-Case.txt\")", last(&p)),
          (Ok(_), _) => "opened, but F_GETPATH failed".to_string(),
        },
      ),
    ))
  }

  fn safe_e_create(root: &Path) -> Res {
    let d = mkdir(root, "e")?;
    let dir = open_path(&d, DIR)?;
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
    let fresh = open_at(dir.0, "new.txt", flags);
    let mode = fresh.as_ref().ok().and_then(|f| stat_fd(f).ok()).map(|s| s.st_mode & 0o777);
    let again = open_at(dir.0, "new.txt", flags);
    fs::write(d.join("victim.txt"), "precious")?;
    symlink(d.join("victim.txt"), d.join("evil.txt"))?;
    let over_link = open_at(dir.0, "evil.txt", flags);
    let victim_kept = fs::read_to_string(d.join("victim.txt")).is_ok_and(|s| s == "precious");
    symlink("nowhere-at-all", d.join("dangling.txt"))?;
    let over_dangling = open_at(dir.0, "dangling.txt", flags);
    let nothing_created = !d.join("nowhere-at-all").exists();
    let pass = fresh.is_ok() && again.as_ref().err() == Some(&Errno(libc::EEXIST)) && over_link.is_err() && victim_kept && over_dangling.is_err() && nothing_created;
    let word = |r: &Result<Fd, Errno>| r.as_ref().err().map_or("created".to_string(), |e| format!("refused ({e})"));
    Ok((
      if pass { Works } else { Fails },
      format!(
        "a new name: {} (mode {})\nthe same name again: {}\na name that is a symlink to an existing file: {}; the file it pointed to is untouched: {}\na name that is a dangling symlink: {}; its target was not created: {}",
        word(&fresh),
        mode.map_or("unknown".to_string(), |m| format!("{m:o}")),
        word(&again),
        word(&over_link),
        yes(victim_kept),
        word(&over_dangling),
        yes(nothing_created),
      ),
    ))
  }

  fn safe_f_apfs_names(root: &Path) -> Res {
    let d = mkdir(root, "f")?;
    fs::write(d.join("Foo.txt"), "x")?;
    let upper = fs::metadata(d.join("foo.TXT"));
    let same_case = upper.as_ref().ok().zip(fs::metadata(d.join("Foo.txt")).ok()).map(|(a, b)| a.ino() == b.ino());
    let case_sensitive = {
      let p = cpath(&d);
      // SAFETY: a valid C string and a documented pathconf name.
      unsafe { libc::pathconf(p.as_ptr(), libc::_PC_CASE_SENSITIVE) }
    };
    let preserving = {
      let p = cpath(&d);
      // SAFETY: as above.
      unsafe { libc::pathconf(p.as_ptr(), libc::_PC_CASE_PRESERVING) }
    };
    let nfc = "caf\u{e9}.txt";
    let nfd = "cafe\u{301}.txt";
    fs::write(d.join(nfc), "x")?;
    let by_nfd = fs::metadata(d.join(nfd));
    let same_form = by_nfd.as_ref().ok().zip(fs::metadata(d.join(nfc)).ok()).map(|(a, b)| a.ino() == b.ino());
    let second = fs::OpenOptions::new().write(true).create_new(true).open(d.join(nfd));
    let stored: Vec<&str> = fs::read_dir(&d)?.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.starts_with("caf")).map(|n| if n == nfc { "NFC" } else if n == nfd { "NFD" } else { "other" }).collect();
    Ok((
      Works,
      format!(
        "pathconf says case-sensitive: {case_sensitive}, case-preserving: {preserving}\na wrong-case name finds the same file: {}\nthe NFD spelling of a name made with NFC finds the same file: {}\ncreating the NFD spelling when the NFC one exists: {}\nnames stored for the two spellings: {}\nSo comparisons of names must fold case and Unicode form the way the volume does, or compare inodes instead.",
        same_case.map_or("no (not found)".to_string(), |s| yes(s).to_string()),
        same_form.map_or("no (not found)".to_string(), |s| yes(s).to_string()),
        match &second {
          Ok(_) => "created a second file (the volume treats them as different names)".to_string(),
          Err(e) => format!("refused ({})", e.kind()),
        },
        stored.join(", "),
      ),
    ))
  }

  fn safe_g_firmlinks() -> Res {
    let home = std::env::var("HOME")?;
    let a = fs::metadata(&home)?;
    let alt = format!("/System/Volumes/Data{home}");
    let b = fs::metadata(&alt);
    let same_home = b.as_ref().ok().map(|b| (b.dev(), b.ino()) == (a.dev(), a.ino()));
    let apps = fs::metadata("/Applications").ok().zip(fs::metadata("/System/Volumes/Data/Applications").ok()).map(|(x, y)| (x.dev(), x.ino()) == (y.dev(), y.ino()));
    let root = fs::metadata("/")?;
    let data = fs::metadata("/System/Volumes/Data");
    let devices_differ = data.as_ref().ok().map(|d| d.dev() != root.dev());
    let mut firmlinked = Vec::new();
    let mut only_data = Vec::new();
    let mut count = 0;
    for e in fs::read_dir("/System/Volumes/Data")?.flatten() {
      let name = e.file_name().to_string_lossy().into_owned();
      if name.starts_with('.') {
        continue;
      }
      count += 1;
      let in_data = fs::symlink_metadata(e.path()).ok();
      let in_root = fs::symlink_metadata(format!("/{name}")).ok();
      match (in_data, in_root) {
        (Some(x), Some(y)) if (x.dev(), x.ino()) == (y.dev(), y.ino()) => firmlinked.push(name),
        _ => only_data.push(name),
      }
    }
    firmlinked.sort();
    only_data.sort();
    firmlinked.truncate(40);
    only_data.truncate(40);
    Ok((
      if same_home == Some(true) { Works } else { Partial },
      format!(
        "the home folder and its /System/Volumes/Data twin are the same directory (same device and inode): {}\n/Applications and its twin: {}\n/ and /System/Volumes/Data are on different devices: {}\n{count} visible entries on the Data volume. The same object at / and on Data (firmlinked): {}\nOnly on the Data volume path: {}\nA disk scan that starts at / and also enters /System/Volumes/Data counts everything twice. Skip /System/Volumes/Data (or compare device and inode) and scan the Data volume through its normal paths.",
        same_home.map_or("could not tell".to_string(), |s| yes(s).to_string()),
        apps.map_or("could not tell".to_string(), |s| yes(s).to_string()),
        devices_differ.map_or("could not tell".to_string(), |s| yes(s).to_string()),
        firmlinked.join(", "),
        only_data.join(", "),
      ),
    ))
  }

  fn safe_h_hardlinks(root: &Path) -> Res {
    let d = mkdir(root, "h")?;
    fs::write(d.join("one.txt"), "data")?;
    fs::hard_link(d.join("one.txt"), d.join("two.txt"))?;
    let (a, b) = (fs::metadata(d.join("one.txt"))?, fs::metadata(d.join("two.txt"))?);
    let linked = a.nlink() == 2 && b.nlink() == 2 && a.ino() == b.ino();
    fs::remove_file(d.join("one.txt"))?;
    let after = fs::metadata(d.join("two.txt"))?;
    let kept = fs::read_to_string(d.join("two.txt")).is_ok_and(|s| s == "data");
    Ok((
      if linked && kept && after.nlink() == 1 { Works } else { Fails },
      format!(
        "two names for one file: nlink {} and {}, same inode: {}\nafter removing one name: the other still reads, nlink {}\nSo a file with nlink above 1 frees no space when one name is deleted, and the duplicate finder must not treat two names of one inode as two copies.",
        a.nlink(),
        b.nlink(),
        yes(a.ino() == b.ino()),
        after.nlink(),
      ),
    ))
  }

  fn safe_i_hidden(root: &Path) -> Res {
    const UF_HIDDEN: u32 = 0x0000_8000;
    const SF_DATALESS: u32 = 0x4000_0000;
    let d = mkdir(root, "i")?;
    fs::write(d.join("flagged.txt"), "x")?;
    fs::write(d.join(".dotfile"), "x")?;
    let p = cpath(&d.join("flagged.txt"));
    // SAFETY: a valid C string; this only sets a flag on a file made in the probe folder.
    let rc = unsafe { libc::chflags(p.as_ptr(), UF_HIDDEN) };
    let set_err = if rc == 0 { None } else { Some(Errno(errno())) };
    let flags = fs::symlink_metadata(d.join("flagged.txt"))?.st_flags();
    let plain = fs::symlink_metadata(d.join(".dotfile"))?.st_flags();
    let dir = open_path(&d, DIR)?;
    let removed = unlink_at(&dir, "flagged.txt").is_ok();
    Ok((
      if set_err.is_none() && flags & UF_HIDDEN != 0 { Works } else { Partial },
      format!(
        "chflags UF_HIDDEN (0x8000): {}; st_flags afterwards 0x{flags:x}, hidden bit set: {}\na dot file has st_flags 0x{plain:x}, so its UF_HIDDEN bit is set: {} (a dot file is hidden by its name only)\nunlinkat removes a file with UF_HIDDEN set: {}\nSF_DATALESS (0x40000000) marks an iCloud placeholder: set on these new files: {}. Reading a placeholder's contents starts a download, so a scan must look at the flag and not open such files.",
        set_err.map_or("worked".to_string(), |e| format!("failed ({e})")),
        yes(flags & UF_HIDDEN != 0),
        yes(plain & UF_HIDDEN != 0),
        yes(removed),
        yes(flags & SF_DATALESS != 0 || plain & SF_DATALESS != 0),
      ),
    ))
  }

  fn safe_deletes(r: &mut Report, root: &Path) {
    r.check("safe-delete a: O_NOFOLLOW refuses a symlink at the end", || safe_a_nofollow(root));
    r.check("safe-delete b: walking from a trusted fd refuses links and '..'", || safe_b_walk(root));
    r.check("safe-delete c: unlinkat on a held directory survives a parent swap", || safe_c_parent_swap(root));
    r.check("safe-delete c2: a name swapped for a link after the check", || safe_c2_file_swap(root));
    r.check("safe-delete d: F_GETPATH, /tmp and wrong-case names", || safe_d_getpath(root));
    r.check("safe-delete e: O_CREAT|O_EXCL|O_NOFOLLOW refuses existing names", || safe_e_create(root));
    r.check("safe-delete f: APFS case and Unicode behaviour", || safe_f_apfs_names(root));
    r.check("safe-delete g: firmlinks and the Data volume", safe_g_firmlinks);
    r.check("safe-delete h: hard links", || safe_h_hardlinks(root));
    r.check("safe-delete i: hidden flags, dot files and iCloud placeholders", || safe_i_hidden(root));
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Trash
  // ---------------------------------------------------------------------------------------------------------------

  const TRASH_MARKER: &str = "mac-probe test file; safe to delete";

  enum Landed {
    Yes(String),
    No,
    Unknown(String),
  }

  fn find_in_trash(trash: &Path, prefix: &str) -> Landed {
    match fs::read_dir(trash) {
      Ok(entries) => match entries.flatten().find(|e| e.file_name().to_string_lossy().starts_with(prefix)) {
        Some(e) => Landed::Yes(e.file_name().to_string_lossy().into_owned()),
        None => Landed::No,
      },
      Err(e) => Landed::Unknown(format!("{} ({})", e.kind(), e)),
    }
  }

  fn trash_method(sb: &Path, unique: &str, label: &str, method: trash::macos::DeleteMethod) -> Res {
    use trash::macos::TrashContextExtMacos;
    let name = format!("{unique}-{label}.txt");
    let path = sb.join(&name);
    fs::write(&path, TRASH_MARKER)?;
    let (tx, rx) = mpsc::channel();
    let target = path.clone();
    // On its own thread with a time limit, so a permission prompt nobody answers cannot hang the run.
    std::thread::Builder::new().name(format!("trash-{label}")).spawn(move || {
      let mut ctx = trash::TrashContext::new();
      ctx.set_delete_method(method);
      let _ = tx.send(ctx.delete(&target).map_err(|e| e.to_string()));
    })?;
    let started = Instant::now();
    let answer = rx.recv_timeout(Duration::from_secs(25));
    let took = started.elapsed().as_secs_f32();
    let home = std::env::var("HOME").unwrap_or_default();
    let trash_dir = Path::new(&home).join(".Trash");
    match answer {
      Ok(Ok(())) => {
        let gone = !path.exists();
        match (gone, find_in_trash(&trash_dir, &format!("{unique}-{label}"))) {
          (true, Landed::Yes(found)) => Ok((Works, format!("the call succeeded in {took:.1} s, the file left the probe folder and is in ~/.Trash as {found}"))),
          (true, Landed::No) => Ok((Partial, format!("the call succeeded in {took:.1} s and the file left the probe folder, but it is not in ~/.Trash"))),
          (true, Landed::Unknown(why)) => Ok((Partial, format!("the call succeeded in {took:.1} s and the file left the probe folder, but ~/.Trash cannot be read here to confirm: {why}"))),
          (false, _) => Ok((Fails, format!("the call said it succeeded in {took:.1} s but the file is still in the probe folder"))),
        }
      }
      Ok(Err(e)) => Ok((Fails, format!("the call failed after {took:.1} s: {e}"))),
      Err(mpsc::RecvTimeoutError::Timeout) => Ok((Fails, "no answer within 25 s; this is what a permission prompt nobody can answer looks like".into())),
      Err(mpsc::RecvTimeoutError::Disconnected) => Ok((Fails, "the trash thread ended without an answer (it crashed)".into())),
    }
  }

  /// Removes from ~/.Trash only the files this run put there: the name starts with this run's unique prefix (it holds the
  /// process id and the time), it is a plain file, and it is tiny.
  fn trash_cleanup(unique: &str) -> Res {
    let home = std::env::var("HOME")?;
    let trash_dir = Path::new(&home).join(".Trash");
    let entries = match fs::read_dir(&trash_dir) {
      Ok(e) => e,
      Err(e) => return Ok((Partial, format!("~/.Trash cannot be read here ({}), so the test files could not be removed; they are named {unique}-*.txt and hold one line of text", e.kind()))),
    };
    let mut removed = 0;
    let mut failed = 0;
    for e in entries.flatten() {
      let name = e.file_name().to_string_lossy().into_owned();
      let small_file = fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_file() && m.len() < 1024);
      if name.starts_with(&format!("{unique}-")) && small_file {
        if fs::remove_file(e.path()).is_ok() { removed += 1 } else { failed += 1 }
      }
    }
    Ok((if failed == 0 { Works } else { Partial }, format!("removed {removed} test file(s) from ~/.Trash; {failed} could not be removed")))
  }

  fn trash_checks(r: &mut Report, sb: &Path) {
    use trash::macos::DeleteMethod;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let unique = format!("shimmy-probe-{}-{nanos}", std::process::id());
    r.check("trash: NsFileManager method (no extra permission)", || trash_method(sb, &unique, "nsfilemanager", DeleteMethod::NsFileManager));
    r.check("trash: Finder method (Apple Events, may need permission)", || trash_method(sb, &unique, "finder", DeleteMethod::Finder));
    r.check("trash: test files removed from ~/.Trash", || trash_cleanup(&unique));
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Connections without root
  // ---------------------------------------------------------------------------------------------------------------

  struct Sock {
    pid: u32,
    uid: u32,
    port: Option<u16>,
    listening: bool,
  }

  /// The local port of an address such as `127.0.0.1:8765`, `*:53`, `[::1]:631` or `1.2.3.4:5->6.7.8.9:443`.
  fn local_port(name: &str) -> Option<u16> {
    name.split("->").next()?.rsplit(':').next()?.parse().ok()
  }

  fn parse_lsof(text: &str) -> Vec<Sock> {
    let (mut pid, mut uid) = (0u32, 0u32);
    let mut cur: Option<Sock> = None;
    let mut out = Vec::new();
    let flush = |cur: &mut Option<Sock>, out: &mut Vec<Sock>| {
      if let Some(s) = cur.take() {
        out.push(s);
      }
    };
    for line in text.lines() {
      let (tag, rest) = line.split_at(line.chars().next().map_or(0, |c| c.len_utf8()));
      match tag {
        "p" => {
          flush(&mut cur, &mut out);
          pid = rest.parse().unwrap_or(0);
        }
        "u" => uid = rest.parse().unwrap_or(0),
        "f" => {
          flush(&mut cur, &mut out);
          cur = Some(Sock { pid, uid, port: None, listening: false });
        }
        "n" => {
          if let Some(s) = cur.as_mut() {
            s.port = local_port(rest);
          }
        }
        "T" => {
          if let Some(s) = cur.as_mut() {
            s.listening |= rest == "ST=LISTEN";
          }
        }
        _ => {}
      }
    }
    flush(&mut cur, &mut out);
    out
  }

  /// Sockets as a summary that holds counts and ports only.
  fn sock_summary(socks: &[Sock], me: u32) -> String {
    let own = socks.iter().filter(|s| s.uid == me).count();
    let root = socks.iter().filter(|s| s.uid == 0).count();
    let uids: BTreeSet<u32> = socks.iter().map(|s| s.uid).collect();
    let pids: BTreeSet<u32> = socks.iter().map(|s| s.pid).collect();
    let others = socks.iter().filter(|s| s.uid != me && s.uid != 0).count();
    let ports: BTreeSet<u16> = socks.iter().filter(|s| s.listening).filter_map(|s| s.port).collect();
    format!(
      "{} sockets from {} processes; owned by this user {own}, by root {root}, by others {others}; {} distinct user ids seen\nlistening ports ({}): {}",
      socks.len(),
      pids.len(),
      uids.len(),
      ports.len(),
      ports.iter().take(30).map(|p| p.to_string()).collect::<Vec<_>>().join(" "),
    )
  }

  fn probe_port() -> Option<u16> {
    std::env::var("PROBE_ROOT_PORT").ok()?.trim().parse().ok()
  }

  fn conn_lsof() -> Res {
    let t = run_tool("/usr/sbin/lsof", &["-nP", "-iTCP", "-iUDP", "-F", "pcnPTuf"], Duration::from_secs(60));
    if !t.started || t.timed_out {
      return Ok((Fails, format!("lsof {}", t.problem())));
    }
    let socks = parse_lsof(&t.stdout);
    let me = unsafe { libc::getuid() };
    let mut lines = vec![format!("lsof exit status: {}", opt(t.status)), sock_summary(&socks, me)];
    lines.push(format!("sockets with a process id: {} of {}", socks.iter().filter(|s| s.pid > 0).count(), socks.len()));
    let mut root_visible = None;
    if let Some(port) = probe_port() {
      let hit = socks.iter().find(|s| s.port == Some(port) && s.listening);
      root_visible = Some(hit.is_some());
      lines.push(match hit {
        Some(s) => format!("root listener on port {port}: SEEN, pid {} uid {}", s.pid, s.uid),
        None => format!("root listener on port {port}: NOT seen"),
      });
    }
    let verdict = if socks.is_empty() {
      Fails
    } else if root_visible == Some(false) || socks.iter().all(|s| s.uid == me) {
      Partial
    } else {
      Works
    };
    Ok((verdict, lines.join("\n")))
  }

  fn conn_netstat() -> Res {
    let t = run_tool("/usr/sbin/netstat", &["-anv", "-p", "tcp"], Duration::from_secs(30));
    if !t.ok() {
      return Ok((Fails, format!("netstat {}", t.problem())));
    }
    let header: Vec<&str> = t.stdout.lines().find(|l| l.starts_with("Proto")).map(|l| l.split_whitespace().collect()).unwrap_or_default();
    // The header has two more words than a row: "Local Address" and "Foreign Address" are two words each.
    let pid_col = header.iter().position(|h| *h == "pid").and_then(|i| i.checked_sub(2));
    let mut rows = 0;
    let mut with_pid = 0;
    let mut listening = BTreeSet::new();
    let mut port_hit: Option<(u32, bool)> = None;
    for line in t.stdout.lines().filter(|l| l.starts_with("tcp")) {
      rows += 1;
      let f: Vec<&str> = line.split_whitespace().collect();
      let pid = pid_col.and_then(|c| f.get(c)).and_then(|v| v.parse::<u32>().ok()).filter(|p| *p > 0).or_else(|| f.iter().skip(5).find_map(|v| v.rsplit_once(':').and_then(|(_, n)| n.parse::<u32>().ok())));
      with_pid += usize::from(pid.is_some());
      let port = f.get(3).and_then(|l| l.rsplit('.').next()).and_then(|p| p.parse::<u16>().ok());
      let is_listen = f.contains(&"LISTEN");
      if is_listen {
        listening.extend(port);
      }
      if is_listen && port.is_some() && port == probe_port() {
        port_hit = Some((pid.unwrap_or(0), pid.is_some()));
      }
    }
    let mut lines = vec![
      format!("tcp rows: {rows}; rows that name a process: {with_pid}; header has a pid column: {}", yes(pid_col.is_some())),
      format!("listening ports ({}): {}", listening.len(), listening.iter().take(30).map(|p| p.to_string()).collect::<Vec<_>>().join(" ")),
    ];
    let mut root_visible = None;
    if let Some(port) = probe_port() {
      root_visible = Some(port_hit.is_some());
      lines.push(match port_hit {
        Some((pid, true)) => {
          let sys = refreshed_processes();
          let owner = sys.process(sysinfo::Pid::from_u32(pid)).and_then(|p| p.user_id().map(|u| **u));
          format!("root listener on port {port}: SEEN with pid {pid} (owner uid per sysinfo: {})", opt(owner))
        }
        Some((_, false)) => format!("root listener on port {port}: seen, but with no process id"),
        None => format!("root listener on port {port}: NOT seen"),
      });
    }
    let verdict = if rows == 0 {
      Fails
    } else if with_pid == 0 || root_visible == Some(false) {
      Partial
    } else {
      Works
    };
    Ok((verdict, lines.join("\n")))
  }

  const PROC_PIDFDSOCKETINFO: c_int = 3;
  const SOCKET_FDINFO_SIZE: usize = 792;

  /// Lists a process's descriptors through libproc: (descriptors, sockets, socket detail readable).
  fn fd_probe(pid: i32) -> Result<(usize, usize, bool), Errno> {
    // SAFETY: the first call only asks for the size; the second fills a buffer of at least that size.
    let needed = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if needed <= 0 {
      return Err(Errno(errno()));
    }
    let mut buf = vec![0u8; needed as usize + 128];
    let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, buf.as_mut_ptr() as *mut c_void, buf.len() as c_int) };
    if got <= 0 {
      return Err(Errno(errno()));
    }
    let (mut fds, mut sockets, mut detail) = (0, 0, false);
    for e in buf[..got as usize].chunks_exact(8) {
      let fd = i32::from_le_bytes([e[0], e[1], e[2], e[3]]);
      let kind = u32::from_le_bytes([e[4], e[5], e[6], e[7]]);
      fds += 1;
      if kind == libc::PROX_FDTYPE_SOCKET as u32 {
        sockets += 1;
        if !detail {
          let mut info = vec![0u8; SOCKET_FDINFO_SIZE + 64];
          // SAFETY: the buffer is larger than the structure for this flavor.
          let n = unsafe { libc::proc_pidfdinfo(pid, fd, PROC_PIDFDSOCKETINFO, info.as_mut_ptr() as *mut c_void, SOCKET_FDINFO_SIZE as c_int) };
          detail = n > 0;
        }
      }
    }
    Ok((fds, sockets, detail))
  }

  fn conn_libproc() -> Res {
    // SAFETY: asking for the count with no buffer, then filling a buffer with room to spare.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
      return Ok((Fails, format!("proc_listallpids failed: {}", Errno(errno()))));
    }
    let mut pids = vec![0i32; count as usize + 64];
    let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * 4) as c_int) };
    let listed = got.max(0) as usize;

    let sys = refreshed_processes();
    let me = unsafe { libc::getuid() };
    let mut own = Vec::new();
    let mut root = Vec::new();
    for p in sys.processes().values() {
      match p.user_id().map(|u| **u) {
        Some(0) if root.len() < 12 => root.push(p.pid().as_u32() as i32),
        Some(u) if u == me && own.len() < 40 => own.push(p.pid().as_u32() as i32),
        _ => {}
      }
    }
    // The root process that holds the probe's listener, when the CI step started one.
    let listener = probe_port().and_then(|port| {
      let socks = parse_lsof(&run_tool("/usr/sbin/lsof", &["-nP", "-iTCP", "-F", "pcnPTuf"], Duration::from_secs(30)).stdout);
      socks.iter().find(|s| s.port == Some(port) && s.listening).map(|s| s.pid as i32)
    });
    if let Some(pid) = listener {
      root.push(pid);
    }

    let mut own_ok = 0;
    let mut own_sockets = 0;
    let mut own_detail = 0;
    for pid in &own {
      if let Ok((_, sockets, detail)) = fd_probe(*pid) {
        own_ok += 1;
        own_sockets += usize::from(sockets > 0);
        own_detail += usize::from(detail);
      }
    }
    let mut root_ok = 0;
    let mut root_errors = BTreeMap::new();
    for pid in &root {
      match fd_probe(*pid) {
        Ok(_) => root_ok += 1,
        Err(e) => *root_errors.entry(e.to_string()).or_insert(0) += 1,
      }
    }
    let lines = [
      format!("proc_listallpids: {listed} process ids"),
      format!("own processes tried {}: descriptors readable for {own_ok}; {own_sockets} of them hold sockets; socket detail (proc_pidfdinfo) readable for {own_detail}", own.len()),
      format!("root processes tried {}: descriptors readable for {root_ok}; errors: {}", root.len(), if root_errors.is_empty() { "none".to_string() } else { root_errors.iter().map(|(e, n)| format!("{e} x{n}")).collect::<Vec<_>>().join("; ") }),
    ];
    let verdict = if own_ok == 0 {
      Fails
    } else if root_ok > 0 && root_ok == root.len() {
      Works
    } else {
      Partial
    };
    Ok((verdict, lines.join("\n")))
  }

  fn connections(r: &mut Report) {
    r.check("connections: lsof without root", conn_lsof);
    r.check("connections: netstat -anv without root", conn_netstat);
    r.check("connections: libproc proc_pidfdinfo, own and root processes", conn_libproc);
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Full Disk Access, disks, start at login, priority
  // ---------------------------------------------------------------------------------------------------------------

  fn full_disk_access() -> Res {
    let home = std::env::var("HOME")?;
    let home = Path::new(&home);
    let targets: [(&str, PathBuf); 7] = [
      ("~/Library/Mail", home.join("Library/Mail")),
      ("~/Library/Safari", home.join("Library/Safari")),
      ("~/Library/Messages", home.join("Library/Messages")),
      ("~/Desktop", home.join("Desktop")),
      ("~/Documents", home.join("Documents")),
      ("~/Downloads", home.join("Downloads")),
      ("/Library/Application Support/com.apple.TCC", PathBuf::from("/Library/Application Support/com.apple.TCC")),
    ];
    let (mut allowed, mut denied, mut absent) = (0, 0, 0);
    let mut lines = Vec::new();
    for (label, path) in &targets {
      let word = match fs::read_dir(path) {
        Ok(_) => {
          allowed += 1;
          "allowed"
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
          absent += 1;
          "not present"
        }
        Err(_) => {
          denied += 1;
          "denied"
        }
      };
      lines.push(format!("{label}: {word}"));
    }
    lines.push("Names inside the folders are never read or recorded. CI runners may have TCC set differently from a normal Mac, for example with Full Disk Access already granted to the runner's shell.".into());
    let verdict = if denied == 0 && absent == 0 {
      Works
    } else if allowed > 0 {
      Partial
    } else if denied > 0 {
      Fails
    } else {
      NotPresent
    };
    Ok((verdict, lines.join("\n")))
  }

  fn statvfs_of(path: &str) -> Option<(u64, u64)> {
    let p = cname(path);
    // SAFETY: an all-zero statvfs is valid to be filled in; `p` is a valid C string.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(p.as_ptr(), &mut s) } != 0 {
      return None;
    }
    Some((s.f_blocks as u64 * s.f_frsize as u64, s.f_bavail as u64 * s.f_frsize as u64))
  }

  fn disks_check() -> Res {
    let disks = Disks::new_with_refreshed_list();
    let mut lines = vec![format!("sysinfo lists {} disks", disks.list().len())];
    let (mut sum_total, mut sum_free) = (0u64, 0u64);
    for d in disks.list() {
      let mount = d.mount_point().to_string_lossy().into_owned();
      // Only system mount points are named; an external drive's name is the user's.
      let shown = if mount == "/" || mount.starts_with("/System/") || mount.starts_with("/private/") { mount } else { "<other volume>".to_string() };
      sum_total += d.total_space();
      sum_free += d.available_space();
      lines.push(format!("  {shown}: total {}, free {}, {}", gb(d.total_space()), gb(d.available_space()), d.file_system().to_string_lossy()));
    }
    let root = statvfs_of("/");
    let data = statvfs_of("/System/Volumes/Data");
    let show = |v: Option<(u64, u64)>| v.map_or("could not be read".to_string(), |(t, f)| format!("total {}, free {}", gb(t), gb(f)));
    lines.push(format!("statvfs /: {}", show(root)));
    lines.push(format!("statvfs /System/Volumes/Data: {}", show(data)));
    lines.push(format!("adding up every disk sysinfo lists gives total {} and free {}", gb(sum_total), gb(sum_free)));
    lines.push("Volume-count trap: on APFS several volumes share one container, so adding up free space over all volumes counts the same space more than once. Home's 'free space' should come from the Data volume (or the volume holding the home folder) on its own.".into());
    Ok((if root.is_some() { Works } else { Partial }, lines.join("\n")))
  }

  fn login_items() -> Res {
    let home = std::env::var("HOME")?;
    let agents = Path::new(&home).join("Library/LaunchAgents");
    let dir = agents.is_dir();
    let plists = fs::read_dir(&agents).map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "plist")).count()).ok();
    let uid = unsafe { libc::getuid() };
    let gui = run_tool("/bin/launchctl", &["print", &format!("gui/{uid}")], Duration::from_secs(15));
    let user = if gui.ok() { None } else { Some(run_tool("/bin/launchctl", &["print", &format!("user/{uid}")], Duration::from_secs(15))) };
    let mut lines = vec![format!("~/Library/LaunchAgents exists: {}{}", yes(dir), plists.map_or(String::new(), |n| format!(" ({n} plist files)")))];
    lines.push(if gui.ok() { format!("launchctl print gui/<uid>: works ({} lines)", gui.stdout.lines().count()) } else { format!("launchctl print gui/<uid>: {}", gui.problem()) });
    if let Some(u) = &user {
      lines.push(if u.ok() { format!("launchctl print user/<uid>: works ({} lines)", u.stdout.lines().count()) } else { format!("launchctl print user/<uid>: {}", u.problem()) });
    }
    lines.push("Nothing was added or changed.".into());
    let print_ok = gui.ok() || user.as_ref().is_some_and(Tool::ok);
    let verdict = match (dir, print_ok) {
      (true, true) => Works,
      (false, false) => Fails,
      _ => Partial,
    };
    Ok((verdict, lines.join("\n")))
  }

  /// Kills and reaps a throwaway child when dropped, so a crash cannot leave it running.
  struct Throwaway(std::process::Child);

  impl Drop for Throwaway {
    fn drop(&mut self) {
      let _ = self.0.kill();
      let _ = self.0.wait();
    }
  }

  fn set_priority(which: c_int, pid: u32, value: c_int) -> Result<(), Errno> {
    // SAFETY: a plain system call on the child this run started.
    if unsafe { libc::setpriority(which, pid, value) } == 0 { Ok(()) } else { Err(Errno(errno())) }
  }

  fn get_nice(pid: u32) -> Option<c_int> {
    // SAFETY: getpriority can return -1 as a real value, so errno is cleared first and read after.
    unsafe {
      *libc::__error() = 0;
      let v = libc::getpriority(libc::PRIO_PROCESS, pid);
      if v == -1 && *libc::__error() != 0 { None } else { Some(v) }
    }
  }

  fn priority_nice() -> Res {
    let child = Throwaway(Command::new("/bin/sleep").arg("30").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?);
    let pid = child.0.id();
    let start = get_nice(pid);
    let lower = set_priority(libc::PRIO_PROCESS, pid, 10);
    let after_lower = get_nice(pid);
    let back = set_priority(libc::PRIO_PROCESS, pid, 0);
    let after_back = get_nice(pid);
    let verdict = match (&lower, &back) {
      (Ok(()), Ok(())) if after_back == Some(0) => Works,
      (Ok(()), Err(_)) => Partial,
      (Ok(()), Ok(())) => Partial,
      _ => Fails,
    };
    Ok((
      verdict,
      format!(
        "throwaway child (/bin/sleep 30) started at nice {}\nsetpriority to 10: {}; nice now {}\nsetpriority back to 0 as a normal user: {}; nice now {}\n{}",
        opt(start),
        lower.map_or_else(|e| format!("refused ({e})"), |_| "worked".into()),
        opt(after_lower),
        back.map_or_else(|e| format!("refused ({e})"), |_| "worked".into()),
        opt(after_back),
        match back {
          Err(_) => "Putting the value back is refused, so Shimmy cannot promise to undo a nice change; the child's value stays until it ends.",
          Ok(()) => "Putting the value back works without root.",
        },
      ),
    ))
  }

  fn priority_background() -> Res {
    let child = Throwaway(Command::new("/bin/sleep").arg("30").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?);
    let pid = child.0.id();
    let down = set_priority(libc::PRIO_DARWIN_PROCESS, pid, libc::PRIO_DARWIN_BG);
    let back = set_priority(libc::PRIO_DARWIN_PROCESS, pid, 0);
    let verdict = match (&down, &back) {
      (Ok(()), Ok(())) => Works,
      (Ok(()), Err(_)) => Partial,
      _ => Fails,
    };
    Ok((
      verdict,
      format!(
        "PRIO_DARWIN_PROCESS with PRIO_DARWIN_BG (the background policy taskpolicy uses) on a throwaway child:\nset: {}\nput back to 0 as a normal user: {}",
        down.map_or_else(|e| format!("refused ({e})"), |_| "worked".into()),
        back.map_or_else(|e| format!("refused ({e})"), |_| "worked".into()),
      ),
    ))
  }

  fn system_misc(r: &mut Report) {
    r.check("full disk access: which protected folders can be listed", full_disk_access);
    r.check("disks: sysinfo, statvfs and the volume-count trap", disks_check);
    r.check("start at login: LaunchAgents and launchctl print (nothing added)", login_items);
    r.check("priority: nice 10 and back to 0 on a throwaway child", priority_nice);
    r.check("priority: Darwin background policy on a throwaway child", priority_background);
  }

  // ---------------------------------------------------------------------------------------------------------------
  // Output
  // ---------------------------------------------------------------------------------------------------------------

  /// The current time in UTC as 2026-11-02 10:00:00.
  fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, tod) = ((secs / 86_400) as i64, secs % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", tod / 3600, tod % 3600 / 60, tod % 60)
  }

  fn counts(r: &Report) -> [(Verdict, usize); 4] {
    [Works, Partial, Fails, NotPresent].map(|v| (v, r.checks.iter().filter(|c| c.result == v).count()))
  }

  fn markdown(r: &Report, when: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Mac probe report\n");
    let _ = writeln!(s, "- **Machine:** {}", r.machine);
    let _ = writeln!(s, "- **Run (UTC):** {when}");
    for n in NOTES {
      let _ = writeln!(s, "- {n}");
    }
    let _ = writeln!(s, "\n## Summary\n\n| Result | Checks |\n|---|---|");
    for (v, n) in counts(r) {
      let _ = writeln!(s, "| {} | {n} |", v.label());
    }
    let _ = writeln!(s, "\n## All checks\n\n| Result | Check | First line of the evidence |\n|---|---|---|");
    for c in &r.checks {
      let first: String = c.evidence.lines().next().unwrap_or("").chars().take(120).collect();
      let _ = writeln!(s, "| {} | {} | {} |", c.result.label(), c.name.replace('|', "/"), first.replace('|', "/"));
    }
    let _ = writeln!(s, "\n## Evidence\n");
    for c in &r.checks {
      let _ = writeln!(s, "### {} - {}\n\nMachine: {}\n\n```text\n{}\n```\n", c.name, c.result.label(), c.machine, c.evidence);
    }
    s
  }

  fn json_report(r: &Report, m: &Machine, when: &str) -> Value {
    json!({
      "tool": "mac-probe",
      "version": env!("CARGO_PKG_VERSION"),
      "generatedUtc": when,
      "machine": {
        "kind": r.machine,
        "hardware": if m.apple_silicon { "Apple Silicon" } else { "Intel" },
        "unameM": m.arch,
        "builtFor": std::env::consts::ARCH,
        "rosetta": m.translated == Some(1),
        "virtualMachine": m.vm.map(|v| v == 1),
        "model": m.model,
        "macos": m.os,
        "cpu": m.cpu,
        "logicalCpus": m.cores,
        "performanceLevels": m.perf_levels,
        "githubActions": m.github,
      },
      "notes": NOTES,
      "summary": counts(r).iter().map(|(v, n)| (v.label().to_string(), json!(n))).collect::<serde_json::Map<String, Value>>(),
      "checks": r.checks.iter().map(|c| json!({ "name": c.name, "result": c.result.label(), "evidence": c.evidence, "machine": c.machine })).collect::<Vec<_>>(),
    })
  }

  /// A fresh folder under the system temp folder, owner-only. It is the only place this run writes or deletes.
  fn make_sandbox() -> std::io::Result<PathBuf> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("mac-probe-{}-{nanos}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&root)?;
    Ok(root)
  }

  /// Removes the probe's own folder, after checking that the path really is one of its own names.
  fn remove_sandbox(root: &Path) -> Res {
    let own_name = root.file_name().is_some_and(|n| n.to_string_lossy().starts_with("mac-probe-"));
    if !own_name || root.parent() != Some(std::env::temp_dir().as_path()) {
      return Ok((Fails, "the folder to remove is not the probe's own temp folder, so nothing was removed".into()));
    }
    // remove_dir_all removes links themselves and never follows them.
    fs::remove_dir_all(root)?;
    Ok((if root.exists() { Fails } else { Works }, format!("the probe's temp folder was removed: {}", yes(!root.exists()))))
  }

  pub fn run() {
    let machine = machine_info();
    let mut r = Report::new(machine.kind());
    eprintln!("mac-probe on {}", r.machine);
    let evidence = machine.evidence();
    r.check("machine", || Ok((if machine.model != "unknown model" && machine.os != "unknown" { Works } else { Partial }, evidence)));

    section(&mut r, "sensors", sensors);
    section(&mut r, "gpu", gpu);
    section(&mut r, "memory", memory);
    section(&mut r, "battery", battery);
    section(&mut r, "processes", processes);

    match make_sandbox() {
      Ok(root) => {
        r.scrub_path(&root, "<probe-folder>");
        eprintln!("safe deletes");
        let sb = root.clone();
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| safe_deletes(&mut r, &sb))) {
          r.add("safe-delete: section", Fails, format!("the section crashed: {}", panic_text(&p)));
        }
        eprintln!("trash");
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| trash_checks(&mut r, &sb))) {
          r.add("trash: section", Fails, format!("the section crashed: {}", panic_text(&p)));
        }
        r.check("cleanup: the probe's own temp folder", || remove_sandbox(&root));
      }
      Err(e) => r.add("safe-delete and trash: temp folder", Fails, format!("could not make the probe's temp folder, so the safe-delete and trash checks did not run: {e}")),
    }

    section(&mut r, "connections", connections);
    section(&mut r, "system", system_misc);

    let when = utc_now();
    let md = markdown(&r, &when);
    let js = serde_json::to_string_pretty(&json_report(&r, &machine, &when)).unwrap_or_default();
    println!("{md}");
    for (file, text) in [(REPORT_MD, md.as_str()), (REPORT_JSON, js.as_str())] {
      if let Err(e) = fs::write(file, text) {
        eprintln!("could not write {file}: {e}");
        std::process::exit(1);
      }
    }
    eprintln!("wrote {REPORT_MD} and {REPORT_JSON}");
  }
}
