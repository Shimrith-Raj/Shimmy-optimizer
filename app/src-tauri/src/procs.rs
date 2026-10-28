//! Process grouping, protection and suggestions. Ported 1:1 from RAM Optimizer v0.2 (server.js) and checked against
//! its output by tests/parity.rs. Keep the strings and ordering identical unless the test fixture is regenerated.
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
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
  /// Protected for a built-in reason, so it cannot be marked Safe.
  pub builtin: bool,
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
  /// Every process is protected for a built-in reason: the page shows a lock and no mark control.
  pub builtin: bool,
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
  /// Protected for a built-in reason (system, shell, security...), not because the user marked it.
  pub builtin: bool,
  pub reason: Option<String>,
}

/// What the user decided about an app. An app with no mark is "default": suggested, never closed automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark {
  /// Never closed (by hand too) and never trimmed.
  Protected,
  /// Clean now and the automatic clean may close it.
  Safe,
}

/// Marks by lower-case exe name, for example "discord.exe".
pub type Marks = BTreeMap<String, Mark>;

pub const MAX_MARKS: usize = 200;
pub const MARKED_REASON: &str = "You marked it protected";

/// Reads marks from settings.json. A key or value it does not know is dropped, so one bad entry cannot reset the rest.
pub fn lenient_marks<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Marks, D::Error> {
  let raw = BTreeMap::<String, String>::deserialize(d)?;
  Ok(raw.into_iter().filter_map(|(k, v)| Some((exe_key(&k).ok()?, parse_mark(&v).ok()??))).take(MAX_MARKS).collect())
}

/// A clean exe name: trimmed, lower-case, ends in ".exe", no folder, at most 260 characters.
pub fn exe_key(name: &str) -> Result<String, String> {
  let k = name.trim().to_lowercase();
  if k.len() > 260 || k.len() <= 4 || !k.ends_with(".exe") || k.contains(['\\', '/', ':', '"', '*', '?', '<', '>', '|']) || k.chars().any(char::is_control) {
    return Err(format!("\"{}\" is not an app name. It should look like discord.exe.", name.chars().take(60).collect::<String>()));
  }
  Ok(k)
}

/// "protected" and "safe" set a mark; "default" (None) removes it.
pub fn parse_mark(s: &str) -> Result<Option<Mark>, String> {
  match s {
    "protected" => Ok(Some(Mark::Protected)),
    "safe" => Ok(Some(Mark::Safe)),
    "default" => Ok(None),
    _ => Err("The mark must be protected, safe or default.".into()),
  }
}

/// Why Shimmy never closes an app with this exe name, when it is protected for a built-in reason.
pub fn builtin_reason(exe: &str) -> Option<String> {
  let p = Proc { pid: 100_000, ppid: 0, name: exe.into(), ws: 0, cmd: String::new(), path: String::new(), title: String::new(), started: None, session: None, cpu: 0.0 };
  let c = classify(&p, &HashSet::new(), &Marks::new());
  if c.builtin { c.reason } else { None }
}

/// Applies a mark to every exe, or to none when any is refused. `blocked` says why an exe can never be marked Safe.
/// Returns the clean exe names that were changed.
pub fn set_marks(marks: &mut Marks, exes: &[String], mark: Option<Mark>, blocked: impl Fn(&str) -> Option<String>) -> Result<Vec<String>, String> {
  if exes.is_empty() || exes.len() > MAX_MARKS {
    return Err(format!("Pick between 1 and {MAX_MARKS} apps."));
  }
  let mut keys: Vec<String> = Vec::new();
  for e in exes {
    let k = exe_key(e)?;
    if mark == Some(Mark::Safe) {
      if let Some(why) = blocked(&k) {
        return Err(format!("{k} cannot be marked Safe: {why}. Shimmy never closes it."));
      }
    }
    if !keys.contains(&k) {
      keys.push(k);
    }
  }
  let mut next = marks.clone();
  for k in &keys {
    match mark {
      Some(m) => next.insert(k.clone(), m),
      None => next.remove(k),
    };
  }
  if next.len() > MAX_MARKS {
    return Err(format!("You can mark up to {MAX_MARKS} apps. Remove some marks first."));
  }
  *marks = next;
  Ok(keys)
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

/// `own` holds Shimmy's own pid and every descendant (its WebView2 processes, the sensor sidecar). A Protected mark
/// protects an app that has no built-in reason; a Safe mark never lifts a built-in protection.
pub fn classify(p: &Proc, own: &HashSet<u32>, marks: &Marks) -> Classified {
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
  let builtin = reason.is_some();
  let reason = reason.or((marks.get(&n) == Some(&Mark::Protected)).then_some(MARKED_REASON));
  Classified {
    app: if reason == Some("Windows system") { "Windows system".into() } else { app_of(p) },
    protected: reason.is_some(),
    builtin,
    reason: reason.map(String::from),
  }
}

pub fn build_groups(procs: &[Proc], own: &HashSet<u32>, marks: &Marks) -> Vec<Group> {
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
    let c = classify(p, own, marks);
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
      builtin: c.builtin,
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
        builtin: g.procs.iter().all(|p| p.builtin),
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

/// What Clean now closes: processes of apps marked Safe that are not protected. An app with a visible window is asked to
/// close (`ask`); one without a window, such as a tray app, is ended (`force`). Unmarked and Protected apps are never in it.
pub struct CleanPlan {
  pub ask: Vec<Proc>,
  pub force: Vec<Proc>,
}

pub fn clean_plan(procs: &[Proc], own: &HashSet<u32>, marks: &Marks) -> CleanPlan {
  let safe: Vec<&Proc> = procs
    .iter()
    .filter(|p| p.pid > 4 && marks.get(&p.name.to_lowercase()) == Some(&Mark::Safe) && !classify(p, own, marks).protected)
    .collect();
  let windowed: HashSet<String> = safe.iter().filter(|p| !p.title.is_empty()).map(|p| p.name.to_lowercase()).collect();
  let (ask, force) = safe.into_iter().cloned().partition(|p| windowed.contains(&p.name.to_lowercase()));
  CleanPlan { ask, force }
}

/// Processes the trim leaves alone: built-in protected ones and apps marked Protected.
pub fn trim_skip(procs: &[Proc], own: &HashSet<u32>, marks: &Marks) -> HashSet<u32> {
  procs.iter().filter(|p| classify(p, own, marks).protected).map(|p| p.pid).collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn proc(pid: u32, name: &str, title: &str) -> Proc {
    Proc { pid, ppid: 1, name: name.into(), ws: 100 << 20, cmd: String::new(), path: String::new(), title: title.into(), started: None, session: Some(1), cpu: 0.0 }
  }

  fn marks(list: &[(&str, Mark)]) -> Marks {
    list.iter().map(|(k, m)| (k.to_string(), *m)).collect()
  }

  fn pids(list: &[Proc]) -> Vec<u32> {
    list.iter().map(|p| p.pid).collect()
  }

  #[test]
  fn protected_mark_protects_an_ordinary_app() {
    let p = proc(10, "Discord.exe", "");
    let plain = classify(&p, &HashSet::new(), &Marks::new());
    assert!(!plain.protected);
    let c = classify(&p, &HashSet::new(), &marks(&[("discord.exe", Mark::Protected)]));
    assert!(c.protected && !c.builtin);
    assert_eq!(c.reason.as_deref(), Some(MARKED_REASON));
  }

  #[test]
  fn safe_mark_never_lifts_a_built_in_protection() {
    let m = marks(&[("svchost.exe", Mark::Safe), ("powershell.exe", Mark::Safe)]);
    for name in ["svchost.exe", "powershell.exe"] {
      let c = classify(&proc(20, name, ""), &HashSet::new(), &m);
      assert!(c.protected && c.builtin, "{name}");
    }
    let c = classify(&proc(21, "discord.exe", ""), &HashSet::new(), &marks(&[("discord.exe", Mark::Safe)]));
    assert!(!c.protected);
  }

  #[test]
  fn built_in_reason_wins_over_a_protected_mark() {
    let c = classify(&proc(22, "explorer.exe", ""), &HashSet::new(), &marks(&[("explorer.exe", Mark::Protected)]));
    assert_eq!(c.reason.as_deref(), Some("Windows system"));
    assert!(c.builtin);
  }

  #[test]
  fn groups_report_marks_and_built_in_locks() {
    let list = [proc(30, "discord.exe", ""), proc(31, "svchost.exe", "")];
    let groups = build_groups(&list, &HashSet::new(), &marks(&[("discord.exe", Mark::Protected)]));
    let d = groups.iter().find(|g| g.app == "discord").unwrap();
    assert!(d.protected && !d.builtin);
    let s = groups.iter().find(|g| g.app == "Windows system").unwrap();
    assert!(s.protected && s.builtin);
  }

  #[test]
  fn clean_plan_takes_only_safe_marked_apps() {
    let list = [
      proc(100, "discord.exe", ""),
      proc(101, "spotify.exe", ""),
      proc(102, "steam.exe", ""),
      proc(103, "onedrive.exe", ""),
      proc(104, "svchost.exe", ""),
      proc(105, "phoneexperiencehost.exe", ""),
    ];
    let m = marks(&[("discord.exe", Mark::Safe), ("spotify.exe", Mark::Protected), ("svchost.exe", Mark::Safe)]);
    let plan = clean_plan(&list, &HashSet::new(), &m);
    // Unmarked (steam, onedrive, the "low risk" Phone Link host) and Protected are left alone; svchost is built-in protected.
    assert_eq!(pids(&plan.force), vec![100]);
    assert!(plan.ask.is_empty());
  }

  #[test]
  fn clean_plan_asks_windowed_apps_and_forces_the_rest() {
    let list = [
      proc(200, "discord.exe", "Discord"),
      proc(201, "discord.exe", ""),
      proc(202, "steam.exe", ""),
      proc(203, "chrome.exe", "Inbox"),
    ];
    let m = marks(&[("discord.exe", Mark::Safe), ("steam.exe", Mark::Safe)]);
    let plan = clean_plan(&list, &HashSet::new(), &m);
    assert_eq!(pids(&plan.ask), vec![200, 201]);
    assert_eq!(pids(&plan.force), vec![202]);
  }

  #[test]
  fn clean_plan_skips_own_and_system_pids() {
    let list = [proc(4, "discord.exe", ""), proc(300, "discord.exe", "")];
    let own: HashSet<u32> = [300].into();
    let plan = clean_plan(&list, &own, &marks(&[("discord.exe", Mark::Safe)]));
    assert!(plan.ask.is_empty() && plan.force.is_empty());
  }

  #[test]
  fn trim_skips_protected_and_marked_processes() {
    let list = [proc(400, "discord.exe", ""), proc(401, "steam.exe", ""), proc(402, "svchost.exe", ""), proc(403, "obs64.exe", "")];
    let skip = trim_skip(&list, &HashSet::new(), &marks(&[("discord.exe", Mark::Protected), ("steam.exe", Mark::Safe)]));
    let mut got: Vec<u32> = skip.into_iter().collect();
    got.sort();
    assert_eq!(got, vec![400, 402, 403]);
  }

  #[test]
  fn exe_names_are_checked() {
    assert_eq!(exe_key("  Discord.EXE ").unwrap(), "discord.exe");
    for bad in ["", ".exe", "discord", "discord.exe.txt", r"C:\x\discord.exe", "a/b.exe", "bad\u{1}.exe", &format!("{}.exe", "a".repeat(257))] {
      assert!(exe_key(bad).is_err(), "{bad:?}");
    }
    assert!(exe_key(&format!("{}.exe", "a".repeat(256))).is_ok());
  }

  #[test]
  fn mark_words_are_checked() {
    assert_eq!(parse_mark("safe"), Ok(Some(Mark::Safe)));
    assert_eq!(parse_mark("protected"), Ok(Some(Mark::Protected)));
    assert_eq!(parse_mark("default"), Ok(None));
    assert!(parse_mark("Safe").is_err() && parse_mark("").is_err());
  }

  #[test]
  fn built_in_apps_cannot_be_marked_safe() {
    for exe in ["svchost.exe", "explorer.exe", "powershell.exe", "msmpeng.exe", "claude.exe", "premiere pro.exe"] {
      assert!(builtin_reason(exe).is_some(), "{exe}");
      let mut m = Marks::new();
      let err = set_marks(&mut m, &[exe.to_string()], Some(Mark::Safe), builtin_reason).unwrap_err();
      assert!(err.contains("cannot be marked Safe"), "{err}");
      assert!(m.is_empty());
    }
    assert!(builtin_reason("discord.exe").is_none());
  }

  #[test]
  fn set_marks_sets_changes_and_removes() {
    let mut m = Marks::new();
    let names = ["Discord.exe".to_string(), "discord.exe".to_string(), "steam.exe".to_string()];
    assert_eq!(set_marks(&mut m, &names, Some(Mark::Safe), builtin_reason).unwrap(), vec!["discord.exe", "steam.exe"]);
    assert_eq!(m.len(), 2);
    set_marks(&mut m, &["steam.exe".to_string()], Some(Mark::Protected), builtin_reason).unwrap();
    assert_eq!(m["steam.exe"], Mark::Protected);
    set_marks(&mut m, &names, None, builtin_reason).unwrap();
    assert!(m.is_empty());
  }

  #[test]
  fn set_marks_is_all_or_nothing_and_capped() {
    let mut m = Marks::new();
    let mixed = ["discord.exe".to_string(), "svchost.exe".to_string()];
    assert!(set_marks(&mut m, &mixed, Some(Mark::Safe), builtin_reason).is_err());
    assert!(m.is_empty());
    assert!(set_marks(&mut m, &["notanapp".to_string()], Some(Mark::Protected), builtin_reason).is_err());
    assert!(set_marks(&mut m, &[], Some(Mark::Safe), builtin_reason).is_err());
    let full: Vec<String> = (0..MAX_MARKS).map(|i| format!("app{i}.exe")).collect();
    set_marks(&mut m, &full, Some(Mark::Protected), builtin_reason).unwrap();
    assert!(set_marks(&mut m, &["one-more.exe".to_string()], Some(Mark::Protected), builtin_reason).is_err());
    assert_eq!(m.len(), MAX_MARKS);
    set_marks(&mut m, &["app0.exe".to_string()], Some(Mark::Safe), builtin_reason).unwrap();
    assert_eq!(m["app0.exe"], Mark::Safe);
  }

  #[test]
  fn settings_reading_drops_what_it_does_not_know() {
    #[derive(Deserialize)]
    struct S {
      #[serde(deserialize_with = "lenient_marks")]
      m: Marks,
    }
    let s: S = serde_json::from_str(r#"{"m":{"Discord.exe":"safe","a.exe":"nope","notexe":"safe","steam.exe":"protected"}}"#).unwrap();
    assert_eq!(s.m, marks(&[("discord.exe", Mark::Safe), ("steam.exe", Mark::Protected)]));
  }
}
