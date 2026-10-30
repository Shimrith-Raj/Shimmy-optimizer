//! Connections page: which app holds which connection, from Windows' own TCP and UDP tables. Read only, with no driver
//! and no firewall changes. Addresses are never looked up, so this page makes no network use of its own.
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use windows::Win32::NetworkManagement::IpHelper::{
  GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
};

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Place {
  #[serde(rename = "This PC")]
  ThisPc,
  #[serde(rename = "Local network")]
  Local,
  #[serde(rename = "Internet")]
  Internet,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conn {
  pub proto: &'static str,
  pub pid: u32,
  pub local: String,
  pub local_port: u16,
  /// None for a port that only waits (listening TCP, and all UDP, which has no connections).
  pub remote: Option<String>,
  pub remote_port: Option<u16>,
  pub place: Option<Place>,
  pub state: &'static str,
}

/// Ports sit in the low 16 bits of the table's DWORD, in network byte order.
fn port(dw: u32) -> u16 {
  u16::from_be(dw as u16)
}

fn v4(dw: u32) -> Ipv4Addr {
  Ipv4Addr::from(dw.to_ne_bytes())
}

fn state_words(state: u32) -> &'static str {
  match state {
    2 => "Listening",
    3 | 4 => "Connecting",
    5 => "Connected",
    1 | 6..=12 => "Closing",
    _ => "Unknown",
  }
}

/// Where an address is. IPv6 forms of an IPv4 address count as that address.
pub fn place(ip: IpAddr) -> Place {
  let ip = match ip {
    IpAddr::V6(a) => a.to_ipv4_mapped().map_or(ip, IpAddr::V4),
    v4 => v4,
  };
  match ip {
    IpAddr::V4(a) if a.is_loopback() || a.is_unspecified() => Place::ThisPc,
    IpAddr::V4(a) if a.is_private() || a.is_link_local() || a.is_multicast() || a.is_broadcast() => Place::Local,
    IpAddr::V6(a) if a.is_loopback() || a.is_unspecified() => Place::ThisPc,
    // fc00::/7 is private, fe80::/10 is link-local, ff00::/8 is multicast.
    IpAddr::V6(a) if a.segments()[0] & 0xfe00 == 0xfc00 || a.segments()[0] & 0xffc0 == 0xfe80 || a.is_multicast() => Place::Local,
    _ => Place::Internet,
  }
}

fn conn(proto: &'static str, pid: u32, local: IpAddr, local_port: u16, remote: Option<(IpAddr, u16)>, state: &'static str) -> Conn {
  Conn { proto, pid, local: local.to_string(), local_port, remote: remote.map(|r| r.0.to_string()), remote_port: remote.map(|r| r.1), place: remote.map(|r| place(r.0)), state }
}

/// A TCP row's far end: a listening socket has none, only the empty address.
fn far_end(state: u32, ip: IpAddr, dw_port: u32) -> Option<(IpAddr, u16)> {
  (state != 2 && !ip.is_unspecified()).then(|| (ip, port(dw_port)))
}

/// The rows of a table as Windows lays it out: a DWORD count, then the rows. A count the buffer cannot hold is cut to what
/// it does hold.
fn rows<T: Copy>(buf: &[u8]) -> Vec<T> {
  if buf.len() < 4 {
    return Vec::new();
  }
  let count = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
  let fit = (buf.len() - 4) / std::mem::size_of::<T>();
  (0..count.min(fit)).map(|i| unsafe { std::ptr::read_unaligned(buf.as_ptr().add(4 + i * std::mem::size_of::<T>()) as *const T) }).collect()
}

/// The two-call dance: ask for the size, then fill. The table can grow in between, so this tries a few times.
fn fetch(call: impl Fn(Option<*mut core::ffi::c_void>, &mut u32) -> u32) -> Result<Vec<u8>, String> {
  let mut size = 0u32;
  for _ in 0..4 {
    let mut buf = vec![0u8; size as usize];
    match call((size > 0).then_some(buf.as_mut_ptr() as *mut _), &mut size) {
      0 => {
        buf.truncate(size as usize);
        return Ok(buf);
      }
      ERROR_INSUFFICIENT_BUFFER => size += 1024,
      e => return Err(format!("Windows did not give the connection table (error {e}).")),
    }
  }
  Err("The connection table kept changing while Shimmy read it. Try again.".into())
}

fn tcp(af: u32) -> Result<Vec<u8>, String> {
  fetch(|p, size| unsafe { GetExtendedTcpTable(p, size, false, af, TCP_TABLE_OWNER_PID_ALL, 0) })
}

fn udp(af: u32) -> Result<Vec<u8>, String> {
  fetch(|p, size| unsafe { GetExtendedUdpTable(p, size, false, af, UDP_TABLE_OWNER_PID, 0) })
}

/// Every TCP and UDP socket on this PC right now, IPv4 and IPv6.
pub fn list() -> Result<Vec<Conn>, String> {
  let mut out = Vec::new();
  for r in rows::<MIB_TCPROW_OWNER_PID>(&tcp(AF_INET)?) {
    let far = far_end(r.dwState, IpAddr::V4(v4(r.dwRemoteAddr)), r.dwRemotePort);
    out.push(conn("TCP", r.dwOwningPid, IpAddr::V4(v4(r.dwLocalAddr)), port(r.dwLocalPort), far, state_words(r.dwState)));
  }
  for r in rows::<MIB_TCP6ROW_OWNER_PID>(&tcp(AF_INET6)?) {
    let far = far_end(r.dwState, IpAddr::V6(Ipv6Addr::from(r.ucRemoteAddr)), r.dwRemotePort);
    out.push(conn("TCP", r.dwOwningPid, IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr)), port(r.dwLocalPort), far, state_words(r.dwState)));
  }
  for r in rows::<MIB_UDPROW_OWNER_PID>(&udp(AF_INET)?) {
    out.push(conn("UDP", r.dwOwningPid, IpAddr::V4(v4(r.dwLocalAddr)), port(r.dwLocalPort), None, "Open"));
  }
  for r in rows::<MIB_UDP6ROW_OWNER_PID>(&udp(AF_INET6)?) {
    out.push(conn("UDP", r.dwOwningPid, IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr)), port(r.dwLocalPort), None, "Open"));
  }
  Ok(out)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConns {
  pub app: String,
  pub internet: usize,
  pub local: usize,
  pub this_pc: usize,
  pub waiting: usize,
  pub conns: Vec<Conn>,
}

/// The name for a socket's owner. Windows files sockets that are closing, once their app has gone, under process 0.
fn owner(pid: u32, names: &HashMap<u32, String>) -> String {
  match (pid, names.get(&pid)) {
    (0, _) => "Closing, app already gone".into(),
    (4, _) => "Windows".into(),
    (_, Some(n)) => n.clone(),
    _ => format!("Process {pid}"),
  }
}

/// Sockets by app: apps on the internet first, then the most connections, then by name.
pub fn by_app(conns: Vec<Conn>, names: &HashMap<u32, String>) -> Vec<AppConns> {
  let mut apps: BTreeMap<String, AppConns> = BTreeMap::new();
  for c in conns {
    let name = owner(c.pid, names);
    let a = apps.entry(name.to_lowercase()).or_insert_with(|| AppConns { app: name, internet: 0, local: 0, this_pc: 0, waiting: 0, conns: Vec::new() });
    match c.place {
      Some(Place::Internet) => a.internet += 1,
      Some(Place::Local) => a.local += 1,
      Some(Place::ThisPc) => a.this_pc += 1,
      None => a.waiting += 1,
    }
    a.conns.push(c);
  }
  let mut out: Vec<AppConns> = apps.into_values().collect();
  // Stable, so equal apps stay in name order. Within an app, the internet comes first too.
  out.sort_by_key(|a| (std::cmp::Reverse(a.internet), std::cmp::Reverse(a.local + a.this_pc)));
  for a in &mut out {
    a.conns.sort_by_key(|c| match c.place {
      Some(Place::Internet) => 0,
      Some(Place::Local) => 1,
      Some(Place::ThisPc) => 2,
      None => 3,
    });
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  fn sock(pid: u32, remote: Option<&str>) -> Conn {
    let far = remote.map(|r| (r.parse().unwrap(), 443));
    conn("TCP", pid, "192.168.1.5".parse().unwrap(), 50_000, far, "Connected")
  }

  #[test]
  fn ports_are_in_network_order() {
    // Port 443 is bytes 01 BB on the wire; Windows leaves junk in the upper half of the DWORD.
    assert_eq!(port(u32::from_ne_bytes([0x01, 0xBB, 0xAA, 0xAA])), 443);
  }

  #[test]
  fn addresses_are_in_network_order() {
    assert_eq!(v4(u32::from_ne_bytes([192, 168, 1, 5])), Ipv4Addr::new(192, 168, 1, 5));
  }

  #[test]
  fn places() {
    let at = |s: &str| place(s.parse().unwrap());
    assert_eq!(at("127.0.0.1"), Place::ThisPc);
    assert_eq!(at("::1"), Place::ThisPc);
    assert_eq!(at("::ffff:127.0.0.1"), Place::ThisPc, "IPv6 form of an IPv4 address");
    assert_eq!(at("192.168.1.9"), Place::Local);
    assert_eq!(at("10.0.0.1"), Place::Local);
    assert_eq!(at("172.16.0.1"), Place::Local);
    assert_eq!(at("169.254.1.1"), Place::Local);
    assert_eq!(at("224.0.0.251"), Place::Local);
    assert_eq!(at("fe80::1"), Place::Local);
    assert_eq!(at("fd00::1"), Place::Local);
    assert_eq!(at("ff02::fb"), Place::Local);
    assert_eq!(at("::ffff:192.168.1.9"), Place::Local);
    assert_eq!(at("172.32.0.1"), Place::Internet, "just outside the private range");
    assert_eq!(at("8.8.8.8"), Place::Internet);
    assert_eq!(at("2606:4700::1111"), Place::Internet);
    assert_eq!(at("::ffff:8.8.8.8"), Place::Internet);
  }

  #[test]
  fn listening_sockets_have_no_far_end() {
    let ip: IpAddr = "8.8.8.8".parse().unwrap();
    assert_eq!(far_end(2, ip, 0), None, "listening");
    assert_eq!(far_end(5, "0.0.0.0".parse().unwrap(), 0), None, "the empty address");
    assert_eq!(far_end(5, ip, u32::from_ne_bytes([0x01, 0xBB, 0, 0])), Some((ip, 443)));
  }

  #[test]
  fn states_in_plain_words() {
    assert_eq!(state_words(2), "Listening");
    assert_eq!(state_words(5), "Connected");
    assert_eq!(state_words(11), "Closing", "TIME_WAIT");
    assert_eq!(state_words(99), "Unknown");
  }

  #[test]
  fn table_rows_are_read_and_a_bad_count_is_cut() {
    let row = |pid: u32| [0u32, 0, pid].iter().flat_map(|d| d.to_ne_bytes()).collect::<Vec<u8>>();
    let mut buf = 2u32.to_ne_bytes().to_vec();
    buf.extend(row(7));
    buf.extend(row(9));
    let got = rows::<MIB_UDPROW_OWNER_PID>(&buf);
    assert_eq!(got.iter().map(|r| r.dwOwningPid).collect::<Vec<_>>(), [7, 9]);

    let mut lies = 1000u32.to_ne_bytes().to_vec();
    lies.extend(row(7));
    assert_eq!(rows::<MIB_UDPROW_OWNER_PID>(&lies).len(), 1, "a count bigger than the buffer");
    assert!(rows::<MIB_UDPROW_OWNER_PID>(&[1, 0]).is_empty());
    assert!(rows::<MIB_UDPROW_OWNER_PID>(&[]).is_empty());
  }

  #[test]
  fn sockets_group_by_app_with_the_internet_first() {
    let names = HashMap::from([(10, "chrome.exe".to_string()), (11, "Chrome.exe".to_string()), (20, "svchost.exe".to_string()), (30, "alpha.exe".to_string())]);
    let apps = by_app(
      vec![
        sock(20, Some("192.168.1.1")),
        sock(30, None),
        sock(10, None),
        sock(11, Some("8.8.8.8")),
        sock(10, Some("127.0.0.1")),
        sock(4, Some("8.8.4.4")),
        sock(0, Some("9.9.9.9")),
        sock(77, None),
      ],
      &names,
    );
    let order: Vec<&str> = apps.iter().map(|a| a.app.as_str()).collect();
    assert_eq!(order, ["chrome.exe", "Closing, app already gone", "Windows", "svchost.exe", "alpha.exe", "Process 77"]);
    let chrome = &apps[0];
    assert_eq!((chrome.internet, chrome.this_pc, chrome.waiting, chrome.conns.len()), (1, 1, 1, 3), "two processes, one app");
    assert_eq!(chrome.conns[0].place, Some(Place::Internet), "the internet first within an app");
    assert_eq!(chrome.conns[2].place, None, "ports that only wait last");
  }
}
