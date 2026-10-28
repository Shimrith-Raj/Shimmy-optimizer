//! Process grouping, protection and suggestions. Ported 1:1 from RAM Optimizer v0.2 (server.js) and checked against
//! its output by tests/parity.rs. Keep the strings and ordering identical unless the test fixture is regenerated.
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

const MB: f64 = 1_048_576.0;

#[derive(Clone, Debug, Deserialize)]
pub struct Proc {
  pub pid: u32,
  pub ppid: u32,
  pub name: String,
  /// Working set in bytes.
  pub ws: u64,
  pub cmd: String,
  pub path: String,
  pub title: String,
  /// Start time, seconds since the Unix epoch.
  pub started: Option<u64>,
  /// Windows session; 0 is services. Unknown in the v0.2 fixture.
  #[serde(default)]
  pub session: Option<u32>,
  /// Share of the whole processor since the last refresh, 0-100.
  #[serde(default)]
  pub cpu: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct GroupProc {
  pub pid: u32,
  pub name: String,
  pub ws: u64,
  pub mb: u64,
  pub title: String,
  pub cmd: String,
  pub started: Option<u64>,
  pub protected: bool,
  pub reason: Option<String>,
  pub cpu: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Group {
  pub app: String,
  pub mb: u64,
  pub count: usize,
  pub procs: Vec<GroupProc>,
  pub protected: bool,
  pub reason: Option<String>,
  pub cpu: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Rec {
  pub id: String,
  pub title: String,
  pub app: Option<String>,
  pub risk: String,
  pub action: String,
  pub pids: Vec<u32>,
  #[serde(rename = "savingsMB")]
  pub savings_mb: u64,
  pub why: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mem {
  #[serde(rename = "totalMB")]
  pub total_mb: u64,
  #[serde(rename = "freeMB")]
  pub free_mb: u64,
  #[serde(rename = "usedMB")]
  pub used_mb: u64,
  pub pct: u64,
}

pub struct Classified {
  pub app: String,
  pub protected: bool,
  pub reason: Option<String>,
}

fn round_mb(bytes: u64) -> u64 {
  (bytes as f64 / MB).round() as u64
}

static SYSTEM: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
  [
    "system", "registry", "secure system", "memory compression", "system idle process", "smss.exe", "csrss.exe",
    "wininit.exe", "services.exe", "lsass.exe", "lsaiso.exe", "svchost.exe", "winlogon.exe", "explorer.exe",
    "dwm.exe", "fontdrvhost.exe", "msmpeng.exe", "nissrv.exe", "securityhealthservice.exe",
    "securityhealthsystray.exe", "searchhost.exe", "searchindexer.exe", "runtimebroker.exe", "taskhostw.exe",
    "sihost.exe", "ctfmon.exe", "audiodg.exe", "spoolsv.exe", "shellexperiencehost.exe",
    "startmenuexperiencehost.exe", "textinputhost.exe", "wudfhost.exe", "dllhost.exe", "sgrmbroker.exe",
    "dashost.exe", "wmiprvse.exe", "applicationframehost.exe", "smartscreen.exe", "useroobebroker.exe",
    "backgroundtaskhost.exe", "unsecapp.exe", "vmmem", "wslservice.exe", "conhost.exe",
  ]
  .into_iter()
  .collect()
});
const SHELLS: [&str; 7] = ["powershell.exe", "pwsh.exe", "cmd.exe", "windowsterminal.exe", "openconsole.exe", "bash.exe", "wsl.exe"];
static EDITORS: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"(?i)premiere|afterfx|after effects|capcut|resolve|photoshop|illustrator|audition|media ?encoder|vegas|blender|obs64|obs32|davinci|lightroom|indesign").unwrap()
});
static ADOBE_APPS: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"(?i)premiere|afterfx|after effects|photoshop|illustrator|audition|media ?encoder|indesign|lightroom").unwrap()
});
static CLIENT_WORK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)salon|shaila|invoice").unwrap());
// New in Shimmy: running elevated, it must also leave security software alone.
static SECURITY: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"(?i)^(mssense|avastsvc|avastui|avgsvc|avgui|avp|avpui|norton|ns\.exe|mcshield|mfemms|bdagent|bdservicehost|mbam|mbamservice|ekrn|egui|sophos|csfalconservice)").unwrap()
});
const BROWSERS: [&str; 3] = ["Brave", "Google Chrome", "Microsoft Edge"];

static FRIENDLY: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
  [
    ("phoneexperiencehost.exe", "Phone Link"), ("crossdeviceservice.exe", "Phone Link"), ("yourphone.exe", "Phone Link"),
    ("crossdeviceresume.exe", "Phone Link"), ("taskmgr.exe", "Task Manager"), ("ccxprocess.exe", "Adobe Creative Cloud"),
    ("creative cloud helper.exe", "Adobe Creative Cloud"), ("adobe desktop service.exe", "Adobe Creative Cloud"),
    ("adobeipcbroker.exe", "Adobe Creative Cloud"), ("coresync.exe", "Adobe Creative Cloud"),
    ("brave.exe", "Brave"), ("chrome.exe", "Google Chrome"), ("msedge.exe", "Microsoft Edge"), ("msedgewebview2.exe", "Edge WebView2"),
  ]
  .into_iter()
  .collect()
});

fn re(pattern: &str) -> Regex {
  Regex::new(pattern).unwrap()
}
static RE_DESKTOP_COMMANDER: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)desktop-commander"));
static RE_SERVER_PDF: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)server-pdf"));
static RE_PREMIERE_MCP: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)premiere-pro-mcp"));
static RE_CAPCUT_MCP: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)capcut-mcp"));
static RE_CREATIVE_CLOUD: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)Creative Cloud"));
static RE_WINDOWS_MCP: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)windows-mcp"));
static RE_CLAUDE_CODE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)claude-code"));
static RE_RENDERER: LazyLock<Regex> = LazyLock::new(|| re(r"--type=renderer"));
static RE_NPX: LazyLock<Regex> = LazyLock::new(|| re(r"npx-cli\.js"));
static RE_DIST_INDEX: LazyLock<Regex> = LazyLock::new(|| re(r"dist[\\/]index\.js"));

pub fn app_of(p: &Proc) -> String {
  let n = p.name.to_lowercase();
  let cmd = &p.cmd;
  if let Some(f) = FRIENDLY.get(n.as_str()) {
    return f.to_string();
  }
  if n.starts_with("claude") {
    return "Claude".into();
  }
  if n == "node.exe" {
    if RE_DESKTOP_COMMANDER.is_match(cmd) { return "MCP · Desktop Commander".into(); }
    if RE_SERVER_PDF.is_match(cmd) { return "MCP · PDF Viewer".into(); }
    if RE_PREMIERE_MCP.is_match(cmd) { return "MCP · Premiere Pro".into(); }
    if RE_CAPCUT_MCP.is_match(cmd) { return "MCP · CapCut".into(); }
    if RE_CREATIVE_CLOUD.is_match(&format!("{cmd} {}", p.path)) { return "Adobe Creative Cloud".into(); }
    return "Node.js".into();
  }
  if n == "python.exe" && RE_WINDOWS_MCP.is_match(cmd) {
    return "MCP · Windows-MCP".into();
  }
  let name = if p.name.is_empty() { "unknown" } else { p.name.as_str() };
  match name.len().checked_sub(4).and_then(|i| name.get(i..).map(|ext| (i, ext))) {
    Some((i, ext)) if ext.eq_ignore_ascii_case(".exe") => name[..i].to_string(),
    _ => name.to_string(),
  }
}

/// `own` holds Shimmy's own pid and every descendant (its WebView2 processes, the sensor sidecar).
pub fn classify(p: &Proc, own: &HashSet<u32>) -> Classified {
  let n = p.name.to_lowercase();
  let reason: Option<&str> = if own.contains(&p.pid) {
    Some("Shimmy Optimizer")
  } else if p.pid == 0 || p.pid == 4 || SYSTEM.contains(n.as_str()) {
    Some("Windows system")
  } else if SHELLS.contains(&n.as_str()) {
    Some("Terminal / shell")
  } else if n.starts_with("claude") || RE_CLAUDE_CODE.is_match(&p.cmd) {
    Some("Claude")
  } else if EDITORS.is_match(&format!("{n} {}", p.path)) {
    Some("Editing app")
  } else if CLIENT_WORK.is_match(&format!("{} {}", p.cmd, p.path)) {
    Some("Salon Shaila work")
  } else if SECURITY.is_match(&n) {
    Some("Security software")
  } else if p.session == Some(0) {
    Some("Windows service")
  } else {
    None
  };
  Classified {
    app: if reason == Some("Windows system") { "Windows system".into() } else { app_of(p) },
    protected: reason.is_some(),
    reason: reason.map(String::from),
  }
}

pub fn build_groups(procs: &[Proc], own: &HashSet<u32>) -> Vec<Group> {
  struct Acc {
    app: String,
    procs: Vec<GroupProc>,
    bytes: u64,
  }
  let mut order: Vec<Acc> = Vec::new();
  let mut index: HashMap<String, usize> = HashMap::new();
  for p in procs {
    if p.pid == 0 {
      continue;
    }
    let c = classify(p, own);
    let i = *index.entry(c.app.clone()).or_insert_with(|| {
      order.push(Acc { app: c.app.clone(), procs: Vec::new(), bytes: 0 });
      order.len() - 1
    });
    let g = &mut order[i];
    g.bytes += p.ws;
    g.procs.push(GroupProc {
      pid: p.pid,
      name: p.name.clone(),
      ws: p.ws,
      mb: round_mb(p.ws),
      title: p.title.clone(),
      cmd: p.cmd.chars().take(300).collect(),
      started: p.started,
      protected: c.protected,
      reason: c.reason,
      cpu: p.cpu,
    });
  }
  let mut groups: Vec<Group> = order
    .into_iter()
    .map(|mut g| {
      g.procs.sort_by(|a, b| b.ws.cmp(&a.ws));
      let mut reasons: Vec<String> = Vec::new();
      for p in g.procs.iter().filter(|p| p.protected) {
        let r = p.reason.clone().unwrap_or_default();
        if !reasons.contains(&r) {
          reasons.push(r);
        }
      }
      let reason = match reasons.len() {
        0 => None,
        1 => Some(reasons.remove(0)),
        _ => Some("Contains protected processes".into()),
      };
      Group {
        cpu: g.procs.iter().map(|p| p.cpu).sum(),
        mb: round_mb(g.bytes),
        count: g.procs.len(),
        protected: g.procs.iter().all(|p| p.protected),
        app: g.app,
        procs: g.procs,
        reason,
      }
    })
    .collect();
  groups.sort_by(|a, b| b.mb.cmp(&a.mb));
  groups
}

struct Item {
  pid: u32,
  mb: u64,
}

pub fn recommend(procs: &[Proc], groups: &[Group], mem: &Mem) -> Vec<Rec> {
  let mut recs: Vec<Rec> = Vec::new();
  let by_app: HashMap<&str, &Group> = groups.iter().map(|g| (g.app.as_str(), g)).collect();
  let mut names: Vec<String> = Vec::new();
  for p in procs {
    let n = p.name.to_lowercase();
    if !names.contains(&n) {
      names.push(n);
    }
  }
  let mut handled: HashSet<String> = HashSet::new();
  let killable = |g: &Group| -> Vec<Item> { g.procs.iter().filter(|p| !p.protected).map(|p| Item { pid: p.pid, mb: p.mb }).collect() };
  let mut add = |id: String, title: String, app: Option<&str>, risk: &str, action: &str, items: Vec<Item>, savings: Option<u64>, why: String| {
    recs.push(Rec {
      id,
      title,
      app: app.map(String::from),
      risk: risk.into(),
      action: action.into(),
      pids: items.iter().map(|i| i.pid).collect(),
      savings_mb: savings.unwrap_or_else(|| items.iter().map(|i| i.mb).sum()),
      why,
    });
  };

  let running: Vec<&str> = BROWSERS.iter().copied().filter(|b| by_app.contains_key(b)).collect();
  if let Some(edge) = by_app.get("Microsoft Edge") {
    let items = killable(edge);
    if !items.is_empty() {
      handled.insert("Microsoft Edge".into());
      let others: Vec<&str> = running.iter().copied().filter(|b| *b != "Microsoft Edge").collect();
      let (risk, why) = if !others.is_empty() {
        ("low", format!("You're also running {}. Edge is most likely idling in the background.", others.join(" and ")))
      } else {
        ("review", "Edge is running. Close it if you are not using it.".to_string())
      };
      add("edge".into(), "Close Microsoft Edge".into(), Some("Microsoft Edge"), risk, "close", items, None, why);
    }
  }
  if running.len() >= 2 {
    let total: u64 = running.iter().map(|b| by_app[b].mb).sum();
    add(
      "multi-browser".into(),
      format!("{} browsers open at once", running.len()),
      None,
      "low",
      "info",
      Vec::new(),
      Some(0),
      format!("{} are using {} MB combined. Sticking to one browser is the single biggest saving available.", running.join(", "), total),
    );
  }

  for b in ["Brave", "Google Chrome"] {
    let Some(g) = by_app.get(b) else { continue };
    for p in g.procs.iter().filter(|p| !p.protected && RE_RENDERER.is_match(&p.cmd) && p.mb > 350).take(3) {
      add(
        format!("renderer-{}", p.pid),
        format!("Heavy {b} tab process ({} MB)", p.mb),
        Some(b),
        "review",
        "close",
        vec![Item { pid: p.pid, mb: p.mb }],
        None,
        "One renderer process holds this much memory. Closing it crashes only the tab(s) it hosts and you can reload them. Often a PDF, video or a heavy web app.".into(),
      );
    }
  }

  if let Some(wv) = by_app.get("Edge WebView2") {
    let items = killable(wv);
    if !items.is_empty() {
      handled.insert("Edge WebView2".into());
      add("webview2".into(), "Edge WebView2 runtimes".into(), Some("Edge WebView2"), "review", "close", items, None,
        "Apps (widgets, Teams, Phone Link, etc.) embed these. Closing them shuts those app panels until you reopen them.".into());
    }
  }

  let mut by_label: Vec<(String, Vec<&Proc>)> = Vec::new();
  for p in procs.iter().filter(|p| p.name.eq_ignore_ascii_case("node.exe") && !RE_NPX.is_match(&p.cmd) && RE_DIST_INDEX.is_match(&p.cmd)) {
    let label = app_of(p);
    if !label.starts_with("MCP") {
      continue;
    }
    match by_label.iter_mut().find(|(l, _)| *l == label) {
      Some((_, list)) => list.push(p),
      None => by_label.push((label, vec![p])),
    }
  }
  for (label, mut list) in by_label {
    if list.len() < 2 {
      continue;
    }
    // Oldest first; unknown start times sort last, as "null" did in v0.2's string sort.
    list.sort_by_key(|p| p.started.unwrap_or(u64::MAX));
    let extras: Vec<Item> = list[..list.len() - 1].iter().map(|p| Item { pid: p.pid, mb: round_mb(p.ws) }).collect();
    handled.insert(label.clone());
    let why = format!(
      "{} copies are running (probably the Claude desktop app and Claude Code each started one). This closes the older {} and keeps the newest. The app that owned the closed one may show the tool as disconnected until it is restarted.",
      list.len(),
      extras.len()
    );
    add(format!("dup-{label}"), format!("Duplicate server: {label}"), Some(&label), "review", "close", extras, None, why);
  }

  let has_premiere = names.iter().any(|n| n.contains("premiere"));
  let has_capcut = names.iter().any(|n| n.contains("capcut"));
  for (label, active) in [("MCP · Premiere Pro", has_premiere), ("MCP · CapCut", has_capcut)] {
    let Some(g) = by_app.get(label) else { continue };
    if active || handled.contains(label) {
      continue;
    }
    handled.insert(label.into());
    let short = label.replace("MCP · ", "");
    add(
      format!("idle-{label}"),
      format!("{short} bridge is idle"),
      Some(label),
      "review",
      "close",
      killable(g),
      None,
      format!("{short} isn't open, so this editing bridge is doing nothing. Restart Claude (or reconnect the server) when you next edit."),
    );
  }

  if let Some(phone) = by_app.get("Phone Link") {
    let items = killable(phone);
    if !items.is_empty() {
      handled.insert("Phone Link".into());
      add("phone".into(), "Close Phone Link background services".into(), Some("Phone Link"), "low", "close", items, None,
        "Phone Link / Cross Device services run in the background even when unused. They start again if you open Phone Link.".into());
    }
  }

  if let Some(cc) = by_app.get("Adobe Creative Cloud") {
    let items = killable(cc);
    if !items.is_empty() && !names.iter().any(|n| ADOBE_APPS.is_match(n)) {
      handled.insert("Adobe Creative Cloud".into());
      add("adobe-cc".into(), "Adobe Creative Cloud background".into(), Some("Adobe Creative Cloud"), "review", "close", items, None,
        "No Adobe app is open, so these helpers are idle. They relaunch on their own when you open Premiere or other Adobe apps.".into());
    }
  }

  if let Some(tm) = by_app.get("Task Manager") {
    let items = killable(tm);
    if !items.is_empty() {
      handled.insert("Task Manager".into());
      add("taskmgr".into(), "Close Task Manager".into(), Some("Task Manager"), "low", "close", items, None,
        "Task Manager uses memory just by being open; this dashboard covers the same ground.".into());
    }
  }

  for g in groups {
    if g.protected || handled.contains(&g.app) || BROWSERS.contains(&g.app.as_str()) || g.mb < 500 {
      continue;
    }
    let items = killable(g);
    if items.is_empty() {
      continue;
    }
    add(format!("heavy-{}", g.app), format!("{} is using {} MB", g.app, g.mb), Some(&g.app), "review", "close", items, None,
      "This app is among the heaviest. Close it if you are not using it. Save your work first.".into());
  }

  if mem.pct >= 70 {
    add("trim".into(), "Trim memory now".into(), None, "low", "trim", Vec::new(), Some(0),
      "Asks Windows to move idle pages out of every app\u{2019}s working set. Nothing is closed. The effect is temporary: apps pull pages back as they need them.".into());
  }

  recs.sort_by(|a, b| b.savings_mb.cmp(&a.savings_mb));
  recs
}
