//! The Rust rules must give exactly what RAM Optimizer v0.2 gave for the same process snapshot.
//! Regenerate the fixture with `node tools/make-fixture.js` (it runs v0.2's own code).
use serde::Deserialize;
use serde_json::{json, Value};
use shimmy_lib::procs::{build_groups, classify, recommend, Mem, Proc};
use std::collections::HashSet;

#[derive(Deserialize)]
struct Snapshot {
  mem: Mem,
  procs: Vec<Proc>,
}

fn snapshot() -> Snapshot {
  serde_json::from_str(include_str!("fixtures/snapshot.json")).unwrap()
}

fn compare(kind: &str, got: &[Value], want: &[Value]) {
  for (i, (g, w)) in got.iter().zip(want).enumerate() {
    assert_eq!(g, w, "{kind} #{i} differs");
  }
  assert_eq!(got.len(), want.len(), "{kind}: count differs");
}

#[test]
fn groups_and_suggestions_match_v02() {
  let snap = snapshot();
  let expected: Value = serde_json::from_str(include_str!("fixtures/expected.json")).unwrap();
  let groups = build_groups(&snap.procs, &HashSet::new());
  let recs = recommend(&snap.procs, &groups, &snap.mem);

  let got_groups: Vec<Value> = groups
    .iter()
    .map(|g| json!({ "app": g.app, "mb": g.mb, "count": g.count, "protected": g.protected, "reason": g.reason,
      "pids": g.procs.iter().map(|p| p.pid).collect::<Vec<_>>() }))
    .collect();
  compare("group", &got_groups, expected["groups"].as_array().unwrap());

  let got_recs: Vec<Value> = recs.iter().map(|r| serde_json::to_value(r).unwrap()).collect();
  compare("suggestion", &got_recs, expected["recs"].as_array().unwrap());
}

fn proc(pid: u32, name: &str, cmd: &str, session: Option<u32>) -> Proc {
  Proc { pid, ppid: 1, name: name.into(), ws: 100 << 20, cmd: cmd.into(), path: String::new(), title: String::new(), started: None, session, cpu: 0.0 }
}

#[test]
fn elevated_mode_protections() {
  let own: HashSet<u32> = [500, 501].into();
  let cases = [
    (proc(501, "msedgewebview2.exe", "", Some(1)), Some("Shimmy Optimizer")),
    (proc(900, "AsusAppService.exe", "", Some(0)), Some("Windows service")),
    (proc(901, "MBAMService.exe", "", Some(1)), Some("Security software")),
    (proc(902, "node.exe", r"C:\Users\x\Desktop\Salon Shaila\invoice\server.js", Some(1)), Some("Salon Shaila work")),
    (proc(903, "PhoneExperienceHost.exe", "", Some(1)), None),
  ];
  for (p, want) in cases {
    let c = classify(&p, &own);
    assert_eq!(c.reason.as_deref(), want, "{}", p.name);
    assert_eq!(c.protected, want.is_some(), "{}", p.name);
  }
}
