//! Updates: reads GitHub's public release info for the latest stable release, downloads its installer and checks the
//! SHA-256 before anything runs. This is the only network use in Shimmy. It all happens here in Rust (the window's CSP
//! allows no network), every request has a timeout, and nothing is uploaded.
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const LATEST: &str = "https://api.github.com/repos/Shimrith-Raj/Shimmy-optimizer/releases/latest";
/// Every release asset must come from here; anything else in the API's answer is ignored.
const DOWNLOADS: &str = "https://github.com/Shimrith-Raj/Shimmy-optimizer/releases/download/";
/// The installer's exact name on a release (the README's download button depends on it too).
const ASSET: &str = "Shimmy-Optimizer-Setup.exe";
/// Checked when the API gives no digest for the installer.
const SUM_ASSET: &str = "Shimmy-Optimizer-Setup.exe.sha256";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const AGENT: &str = concat!("Shimmy-Optimizer/", env!("CARGO_PKG_VERSION"));
const NOTES_MAX: usize = 20_000;
/// A download larger than this is refused when the release did not say how big it is.
const SIZE_CAP: u64 = 1 << 30;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Release {
  pub current: String,
  pub latest: String,
  pub newer: bool,
  pub name: String,
  pub published: String,
  /// The release notes as GitHub has them (Markdown). The page shows them as plain text, never as HTML.
  pub notes: String,
  #[serde(skip)]
  pub asset_url: Option<String>,
  pub asset_size: u64,
  /// The SHA-256 the installer must match, lower case. None means it cannot be checked.
  pub sha256: Option<String>,
  /// Why this update cannot be installed from here, in plain words.
  pub problem: Option<String>,
  /// When the check ran (ms since epoch).
  pub checked_at: u64,
}

/// A downloaded installer whose hash matched.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Downloaded {
  pub version: String,
  #[serde(skip)]
  pub path: PathBuf,
  #[serde(skip)]
  pub sha256: String,
  pub bytes: u64,
}

struct Parsed {
  release: Release,
  /// The `.sha256` asset, when the installer has no digest in the API's answer.
  sum_url: Option<String>,
}

/// "v0.5.1" or "0.5.1" as (major, minor, patch). Missing parts count as 0; anything that is not plain numbers (a
/// pre-release suffix, a fourth part) is not a version Shimmy compares.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
  let s = s.trim();
  let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
  let (mut out, mut n) = ([0u64; 3], 0);
  for part in s.split('.') {
    if n == 3 || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
      return None;
    }
    out[n] = part.parse().ok()?;
    n += 1;
  }
  Some((out[0], out[1], out[2]))
}

pub fn is_newer(current: &str, latest: &str) -> bool {
  matches!((parse_version(current), parse_version(latest)), (Some(c), Some(l)) if l > c)
}

/// The newest version seen on GitHub when it is newer than this one (the rail marker).
pub fn pending(latest_seen: &str) -> Option<String> {
  is_newer(CURRENT, latest_seen).then(|| latest_seen.to_string())
}

fn is_sha256(s: &str) -> bool {
  s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The API's `"digest": "sha256:<hex>"`.
fn digest_hex(d: &str) -> Option<String> {
  d.trim().strip_prefix("sha256:").filter(|h| is_sha256(h)).map(str::to_ascii_lowercase)
}

/// The first hex token of a `.sha256` file (`<hex>`, `<hex>  name` or `<hex> *name`).
fn parse_checksum(text: &str) -> Option<String> {
  let first = text.trim_start_matches('\u{feff}').split_whitespace().next()?;
  is_sha256(first).then(|| first.to_ascii_lowercase())
}

fn sha256_reader(r: &mut impl Read) -> std::io::Result<String> {
  let mut h = Sha256::new();
  let mut buf = vec![0u8; 64 * 1024];
  loop {
    let n = r.read(&mut buf)?;
    if n == 0 {
      break;
    }
    h.update(&buf[..n]);
  }
  Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn sha256_file(p: &Path) -> std::io::Result<String> {
  sha256_reader(&mut std::fs::File::open(p)?)
}

/// Where a version's installer is saved.
pub fn installer_path(version: &str) -> PathBuf {
  std::env::temp_dir().join(format!("Shimmy-Optimizer-Setup-{version}.exe"))
}

fn parse_release(v: &Value, current: &str) -> Result<Parsed, String> {
  let tag = v["tag_name"].as_str().unwrap_or_default();
  let (a, b, c) = parse_version(tag).ok_or_else(|| format!("The newest release is tagged \"{tag}\", which is not a version number Shimmy understands."))?;
  let latest = format!("{a}.{b}.{c}");
  let assets = v["assets"].as_array().map(Vec::as_slice).unwrap_or_default();
  let asset = |name: &str| assets.iter().find(|x| x["name"] == name);
  let url_of = |x: &Value| x["browser_download_url"].as_str().filter(|u| u.starts_with(DOWNLOADS)).map(str::to_string);
  let installer = asset(ASSET);
  let sha256 = installer.and_then(|x| x["digest"].as_str()).and_then(digest_hex);
  let sum_url = if sha256.is_none() { asset(SUM_ASSET).and_then(url_of) } else { None };
  let mut notes: String = v["body"].as_str().unwrap_or_default().chars().take(NOTES_MAX).collect();
  notes = notes.replace("\r\n", "\n");
  let mut release = Release {
    current: current.to_string(),
    newer: is_newer(current, &latest),
    name: v["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or(tag).to_string(),
    published: v["published_at"].as_str().unwrap_or_default().to_string(),
    notes,
    asset_url: installer.and_then(url_of),
    asset_size: installer.and_then(|x| x["size"].as_u64()).unwrap_or(0),
    sha256,
    problem: None,
    latest,
    checked_at: crate::store::now_ms(),
  };
  set_problem(&mut release);
  Ok(Parsed { release, sum_url })
}

/// A newer release Shimmy cannot install is explained, never installed without a checksum.
fn set_problem(r: &mut Release) {
  r.problem = if !r.newer {
    None
  } else if r.asset_url.is_none() {
    Some(format!("This release has no {ASSET} attached, so Shimmy cannot install it."))
  } else if r.sha256.is_none() {
    Some("This release has no SHA-256 checksum that Shimmy can read, so it will not install it. Shimmy never runs an installer it cannot check.".to_string())
  } else {
    None
  };
}

/// What an HTTP status from GitHub means for the person reading it.
fn status_message(code: u16, rate_limited: bool) -> String {
  match code {
    429 => "GitHub is limiting requests from this connection right now. Try again in an hour.".into(),
    403 if rate_limited => "GitHub is limiting requests from this connection right now. Try again in an hour.".into(),
    404 => "No release has been published yet.".into(),
    c => format!("GitHub answered with an error ({c}). Try again later."),
  }
}

fn net_error(e: ureq::Error) -> String {
  match e {
    ureq::Error::Timeout(_) => "GitHub took too long to answer. Try again in a moment.".into(),
    ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Io(_) => {
      "Shimmy could not reach GitHub. Check that this PC is online, then try again.".into()
    }
    other => format!("The connection to GitHub failed ({other})."),
  }
}

fn agent(total: Duration) -> ureq::Agent {
  ureq::Agent::new_with_config(
    ureq::Agent::config_builder()
      .timeout_connect(Some(Duration::from_secs(10)))
      .timeout_recv_response(Some(Duration::from_secs(20)))
      .timeout_global(Some(total))
      .http_status_as_error(false)
      .https_only(true)
      .build(),
  )
}

fn get(agent: &ureq::Agent, url: &str, accept: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
  let resp = agent.get(url).header("User-Agent", AGENT).header("Accept", accept).call().map_err(net_error)?;
  let code = resp.status().as_u16();
  if !(200..300).contains(&code) {
    let rate_limited = resp.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()) == Some("0");
    return Err(status_message(code, rate_limited));
  }
  Ok(resp)
}

fn text_of(mut resp: ureq::http::Response<ureq::Body>) -> Result<String, String> {
  resp.body_mut().read_to_string().map_err(net_error)
}

/// Reads the latest stable release (GitHub's "latest" never returns a pre-release or a draft).
pub fn check() -> Result<Release, String> {
  let a = agent(Duration::from_secs(30));
  let text = text_of(get(&a, LATEST, "application/vnd.github+json")?)?;
  let json: Value = serde_json::from_str(&text).map_err(|_| "GitHub's answer was not understood.".to_string())?;
  let mut p = parse_release(&json, CURRENT)?;
  if p.release.newer && p.release.sha256.is_none() {
    if let Some(url) = &p.sum_url {
      p.release.sha256 = get(&a, url, "text/plain").and_then(text_of).ok().and_then(|t| parse_checksum(&t));
      set_problem(&mut p.release);
    }
  }
  Ok(p.release)
}

/// Downloads the installer to the temp folder, checks its SHA-256 against the release's, and only then gives it its
/// real name. A mismatch deletes the file. `done` and `total` are bytes, for the page's progress bar.
pub fn download(rel: &Release, done: &AtomicU64, total: &AtomicU64) -> Result<Downloaded, String> {
  if let Some(p) = &rel.problem {
    return Err(p.clone());
  }
  let url = rel.asset_url.as_deref().ok_or("This release has no installer to download.")?;
  let want = rel.sha256.as_deref().ok_or("This release has no checksum, so Shimmy will not download it.")?;
  let path = installer_path(&rel.latest);
  let part = PathBuf::from(format!("{}.part", path.display()));
  done.store(0, Ordering::Relaxed);
  total.store(rel.asset_size, Ordering::Relaxed);
  let fail = |why: &str| -> String {
    let _ = std::fs::remove_file(&part);
    why.to_string()
  };

  let a = agent(Duration::from_secs(20 * 60));
  let mut resp = get(&a, url, "application/octet-stream")?;
  if rel.asset_size == 0 {
    let len = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    total.store(len, Ordering::Relaxed);
  }
  let limit = if rel.asset_size > 0 { rel.asset_size } else { SIZE_CAP };
  let mut file = std::fs::File::create(&part).map_err(|e| format!("Shimmy could not save the installer to the temp folder: {e}"))?;
  let mut reader = resp.body_mut().as_reader();
  let mut buf = vec![0u8; 64 * 1024];
  let mut got = 0u64;
  loop {
    let n = reader.read(&mut buf).map_err(|_| fail("The download was interrupted. Check the connection and try again."))?;
    if n == 0 {
      break;
    }
    got += n as u64;
    if got > limit {
      return Err(fail("The download is larger than the release says, so it was stopped and deleted."));
    }
    file.write_all(&buf[..n]).map_err(|e| fail(&format!("Shimmy could not write the installer to the temp folder: {e}")))?;
    done.store(got, Ordering::Relaxed);
  }
  file.flush().map_err(|e| fail(&format!("Shimmy could not write the installer to the temp folder: {e}")))?;
  drop(file);
  if rel.asset_size > 0 && got != rel.asset_size {
    return Err(fail("The download ended early, so it was deleted. Try again."));
  }
  let have = sha256_file(&part).map_err(|e| fail(&format!("Shimmy could not read the download back to check it: {e}")))?;
  if have != want {
    return Err(fail("The download does not match its published SHA-256 checksum, so it was deleted and will not be installed. Try again; if it keeps happening, do not install it."));
  }
  let _ = std::fs::remove_file(&path);
  std::fs::rename(&part, &path).map_err(|e| fail(&format!("Shimmy could not finish saving the installer: {e}")))?;
  Ok(Downloaded { version: rel.latest.clone(), path, sha256: have, bytes: got })
}

/// Starts the installer (its normal window, not silent), after hashing the file again: it has sat in the temp folder
/// since the download. The caller exits Shimmy right after.
pub fn install(d: &Downloaded) -> Result<(), String> {
  let have = sha256_file(&d.path).map_err(|_| "The downloaded installer is gone. Download it again.".to_string())?;
  if have != d.sha256 {
    let _ = std::fs::remove_file(&d.path);
    return Err("The downloaded installer changed after it was checked, so it was deleted. Download it again.".into());
  }
  // A job object from the Task Scheduler start could end the installer with Shimmy; leave it if the job allows.
  let run = |flags: u32| std::process::Command::new(&d.path).creation_flags(flags).spawn();
  run(CREATE_BREAKAWAY_FROM_JOB).or_else(|_| run(0)).map(|_| ()).map_err(|e| format!("Windows would not start the installer: {e}"))
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn versions_compare_numerically() {
    assert_eq!(parse_version("v0.5.1"), Some((0, 5, 1)));
    assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
    assert_eq!(parse_version(" V1.2 "), Some((1, 2, 0)));
    assert_eq!(parse_version("2"), Some((2, 0, 0)));
    for bad in ["", "v", "1.2.3.4", "1.x.3", "1.2.3-beta", "1..2", "latest"] {
      assert_eq!(parse_version(bad), None, "{bad}");
    }
    assert!(is_newer("0.5.0", "v0.5.1"));
    assert!(is_newer("0.9.9", "0.10.0"), "10 is more than 9, not a text comparison");
    assert!(is_newer("0.5.9", "1.0.0"));
    assert!(!is_newer("0.5.0", "0.5.0"));
    assert!(!is_newer("0.5.1", "v0.5.0"));
    assert!(!is_newer("0.5.0", "nightly"), "an unreadable tag is never an update");
    assert!(!is_newer("junk", "0.5.1"));
  }

  const HEX: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

  #[test]
  fn checksums() {
    assert_eq!(digest_hex(&format!("sha256:{HEX}")), Some(HEX.into()));
    assert_eq!(digest_hex(&format!("sha256:{}", HEX.to_uppercase())), Some(HEX.into()));
    assert_eq!(digest_hex("sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709"), None);
    assert_eq!(digest_hex("sha256:abc"), None);
    assert_eq!(parse_checksum(HEX), Some(HEX.into()));
    assert_eq!(parse_checksum(&format!("{HEX}  Shimmy-Optimizer-Setup.exe\n")), Some(HEX.into()));
    assert_eq!(parse_checksum(&format!("\u{feff}{} *Shimmy-Optimizer-Setup.exe\r\n", HEX.to_uppercase())), Some(HEX.into()));
    assert_eq!(parse_checksum("not a hash at all"), None);
    assert_eq!(parse_checksum(""), None);
    assert_eq!(parse_checksum(&HEX[..63]), None);
  }

  #[test]
  fn hashes_bytes() {
    assert_eq!(sha256_reader(&mut std::io::Cursor::new(b"abc")).unwrap(), HEX);
    assert_eq!(sha256_reader(&mut std::io::Cursor::new(b"")).unwrap(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
  }

  fn sample(assets: Value) -> Value {
    json!({ "tag_name": "v0.5.1", "name": "Shimmy 0.5.1", "published_at": "2026-11-02T10:00:00Z", "body": "- Fixed a thing\r\n- Added another", "assets": assets })
  }

  fn asset(name: &str, extra: Value) -> Value {
    let mut a = json!({ "name": name, "size": 41_000_000u64, "browser_download_url": format!("{DOWNLOADS}v0.5.1/{name}") });
    a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    a
  }

  #[test]
  fn reads_a_release_with_a_digest() {
    let v = sample(json!([asset(ASSET, json!({ "digest": format!("sha256:{HEX}") })), asset("other.zip", json!({}))]));
    let p = parse_release(&v, "0.5.0").unwrap();
    let r = p.release;
    assert_eq!((r.latest.as_str(), r.newer, r.name.as_str()), ("0.5.1", true, "Shimmy 0.5.1"));
    assert_eq!(r.sha256.as_deref(), Some(HEX));
    assert_eq!(r.asset_size, 41_000_000);
    assert_eq!(r.notes, "- Fixed a thing\n- Added another");
    assert_eq!(r.problem, None);
    assert_eq!(r.asset_url.as_deref(), Some(format!("{DOWNLOADS}v0.5.1/{ASSET}").as_str()));
    assert_eq!(p.sum_url, None);
  }

  #[test]
  fn falls_back_to_the_sha256_asset() {
    let v = sample(json!([asset(ASSET, json!({ "digest": null })), asset(SUM_ASSET, json!({}))]));
    let mut p = parse_release(&v, "0.5.0").unwrap();
    assert_eq!(p.release.sha256, None);
    assert!(p.release.problem.as_deref().unwrap().contains("no SHA-256 checksum"), "until the file is read");
    assert_eq!(p.sum_url.as_deref(), Some(format!("{DOWNLOADS}v0.5.1/{SUM_ASSET}").as_str()));
    p.release.sha256 = parse_checksum(&format!("{HEX}  {ASSET}"));
    set_problem(&mut p.release);
    assert_eq!(p.release.problem, None);
  }

  #[test]
  fn refuses_what_it_cannot_check() {
    let none = parse_release(&sample(json!([asset(ASSET, json!({}))])), "0.5.0").unwrap().release;
    assert_eq!(none.sha256, None);
    assert!(none.problem.is_some(), "no digest and no .sha256 asset: refuse to install");
    let missing = parse_release(&sample(json!([])), "0.5.0").unwrap().release;
    assert!(missing.problem.as_deref().unwrap().contains(ASSET));
    // The same release is no problem for someone who already has it.
    assert_eq!(parse_release(&sample(json!([])), "0.5.1").unwrap().release.problem, None);
    // An asset served from anywhere but this repository's release downloads is ignored.
    let elsewhere = asset(ASSET, json!({ "browser_download_url": "https://example.com/evil.exe", "digest": format!("sha256:{HEX}") }));
    assert_eq!(parse_release(&sample(json!([elsewhere])), "0.5.0").unwrap().release.asset_url, None);
    assert!(parse_release(&json!({ "tag_name": "nightly" }), "0.5.0").is_err());
  }

  #[test]
  fn up_to_date_and_ahead_are_not_newer() {
    let v = sample(json!([asset(ASSET, json!({ "digest": format!("sha256:{HEX}") }))]));
    assert!(!parse_release(&v, "0.5.1").unwrap().release.newer);
    assert!(!parse_release(&v, "0.6.0").unwrap().release.newer);
    assert_eq!(pending("0.0.1"), None);
    assert_eq!(pending(""), None);
  }

  #[test]
  fn http_errors_in_plain_words() {
    assert!(status_message(403, true).contains("limiting"));
    assert!(status_message(429, false).contains("limiting"));
    assert!(status_message(403, false).contains("403"));
    assert!(status_message(404, false).contains("No release"));
    assert!(status_message(500, false).contains("500"));
  }

  #[test]
  fn installer_name_carries_the_version() {
    assert!(installer_path("0.5.1").ends_with("Shimmy-Optimizer-Setup-0.5.1.exe"));
  }
}
