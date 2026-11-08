//! macOS: the kernel's 64-bit byte counters (`sysctl NET_RT_IFLIST2`), what
//! each adapter is (`networksetup -listallhardwareports`), the one in use
//! (`route -n get default`), an Ethernet's speed (`ifconfig`'s media line) and
//! the Wi-Fi (`system_profiler SPAirPortDataType`). Since macOS 14.4 the
//! Wi-Fi's name is shown to apps only with Location Services allowed.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use super::{Iface, Kind, WifiInfo};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `networksetup -listallhardwareports` → device ("en0") → port ("Wi-Fi").
pub(crate) fn parse_hardware_ports(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut port: Option<String> = None;
    for l in text.lines() {
        if let Some(p) = l.strip_prefix("Hardware Port:") {
            port = Some(p.trim().to_string());
        } else if let Some(d) = l.strip_prefix("Device:") {
            if let Some(p) = port.take() {
                out.insert(d.trim().to_string(), p);
            }
        }
    }
    out
}

pub(crate) fn kind_of_port(port: &str) -> Kind {
    let p = port.to_lowercase();
    if p == "wi-fi" || p == "airport" {
        Kind::Wifi
    } else if p.contains("iphone") || p.contains("ipad") {
        Kind::Cellular
    } else if p.contains("bridge") || p.contains("bluetooth") {
        Kind::Other
    } else if p.contains("ethernet") || p.contains("lan") || p.contains("thunderbolt") {
        Kind::Ethernet
    } else {
        Kind::Other
    }
}

/// `route -n get default` → "interface: en0".
pub(crate) fn parse_route_get(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.trim().strip_prefix("interface:").map(|i| i.trim().to_string())).filter(|i| !i.is_empty())
}

/// `ifconfig en7`'s "media: autoselect (1000baseT <full-duplex>)" → 1000 (Mb/s).
pub(crate) fn parse_media_mbps(ifconfig: &str) -> Option<u64> {
    let line = ifconfig.lines().find(|l| l.trim_start().starts_with("media:"))?;
    let inner = line.split('(').nth(1)?;
    let lower = inner.to_lowercase();
    let at = lower.find("base")?;
    let num = &lower[..at];
    if let Some(g) = num.strip_suffix('g') {
        return g.trim().parse::<u64>().ok().map(|g| g * 1000);
    }
    num.trim().parse().ok()
}

/// `system_profiler SPAirPortDataType -json` → the connected Wi-Fi.
pub(crate) fn parse_airport_json(json: &str) -> Option<WifiInfo> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    for top in v["SPAirPortDataType"].as_array()? {
        for i in top["spairport_airport_interfaces"].as_array().into_iter().flatten() {
            let cur = &i["spairport_current_network_information"];
            if !cur.is_object() {
                continue;
            }
            let name = cur["_name"].as_str().unwrap_or_default();
            let redacted = name.is_empty() || name.contains("redacted");
            let rssi = cur["spairport_signal_noise"].as_str().and_then(|s| s.split_whitespace().next()).and_then(|x| x.parse::<i32>().ok());
            let rate = cur["spairport_network_rate"].as_f64();
            return Some(WifiInfo {
                ssid: (!redacted).then(|| name.to_string()),
                signal: rssi.map(|r| ((r + 100) * 2).clamp(0, 100) as u32),
                rssi_dbm: rssi,
                rx_mbps: rate,
                tx_mbps: None,
                note: redacted.then(|| "macOS shows the Wi-Fi's name only to apps allowed in Location Services.".to_string()),
                fix: redacted.then(|| "location".to_string()),
            });
        }
    }
    None
}

/// Slow answers, asked for again only now and then.
struct Cache {
    ports: Option<(Instant, HashMap<String, String>)>,
    route: Option<(Instant, Option<String>)>,
    media: HashMap<String, (Instant, Option<u64>)>,
}

fn cache() -> &'static Mutex<Cache> {
    static C: std::sync::OnceLock<Mutex<Cache>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(Cache { ports: None, route: None, media: HashMap::new() }))
}

fn run(program: &str, args: &[&str]) -> String {
    crate::spawn::command(program).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn counters() -> Vec<(String, u64, u64, bool)> {
    let mut out = Vec::new();
    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
    let mut len: libc::size_t = 0;
    // SAFETY: sysctl is asked for the size, then fills a buffer of that size;
    // each message is read unaligned within the bytes it wrote.
    unsafe {
        if libc::sysctl(mib.as_mut_ptr(), 6, std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) != 0 || len == 0 {
            return out;
        }
        let mut buf = vec![0u8; len];
        if libc::sysctl(mib.as_mut_ptr(), 6, buf.as_mut_ptr() as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0) != 0 {
            return out;
        }
        let mut off = 0usize;
        while off + 4 <= len {
            let msglen = u16::from_ne_bytes([buf[off], buf[off + 1]]) as usize;
            let kind = buf[off + 3] as i32;
            if msglen == 0 {
                break;
            }
            if kind == libc::RTM_IFINFO2 && off + std::mem::size_of::<libc::if_msghdr2>() <= len {
                let m: libc::if_msghdr2 = std::ptr::read_unaligned(buf.as_ptr().add(off) as *const libc::if_msghdr2);
                let index = m.ifm_index;
                let flags = m.ifm_flags;
                let data = m.ifm_data;
                let mut name = [0 as libc::c_char; libc::IF_NAMESIZE];
                if !libc::if_indextoname(index as libc::c_uint, name.as_mut_ptr()).is_null() {
                    let n = std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy().to_string();
                    let up = flags & libc::IFF_UP != 0 && flags & libc::IFF_RUNNING != 0;
                    out.push((n, data.ifi_ibytes, data.ifi_obytes, up));
                }
            }
            off += msglen;
        }
    }
    out
}

#[cfg(not(target_os = "macos"))]
fn counters() -> Vec<(String, u64, u64, bool)> {
    Vec::new()
}

pub fn read() -> Vec<Iface> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let raw = counters();
    let (ports, route) = {
        let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
        if c.ports.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(60)) {
            c.ports = Some((Instant::now(), parse_hardware_ports(&run("networksetup", &["-listallhardwareports"]))));
        }
        if c.route.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(10)) {
            c.route = Some((Instant::now(), parse_route_get(&run("route", &["-n", "get", "default"]))));
        }
        (c.ports.as_ref().map(|p| p.1.clone()).unwrap_or_default(), c.route.as_ref().and_then(|r| r.1.clone()))
    };
    raw.into_iter()
        .filter(|(n, _, _, _)| n != "lo0")
        .map(|(name, rx, tx, up)| {
            let port = ports.get(&name).cloned();
            let kind = port.as_deref().map(kind_of_port).unwrap_or(Kind::Virtual);
            let link_bps = if kind == Kind::Ethernet && up {
                let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
                let fresh = c.media.get(&name).is_some_and(|(t, _)| t.elapsed() < Duration::from_secs(60));
                if !fresh {
                    let m = parse_media_mbps(&run("ifconfig", &[&name]));
                    c.media.insert(name.clone(), (Instant::now(), m));
                }
                c.media.get(&name).and_then(|(_, m)| *m).map(|m| m * 1_000_000)
            } else {
                None
            };
            Iface {
                id: name.clone(),
                in_use: up && route.as_deref() == Some(name.as_str()),
                detail: port.clone(),
                name: match &port {
                    Some(p) => format!("{} ({})", p, name),
                    None => name.clone(),
                },
                kind,
                up,
                rx,
                tx,
                link_bps,
            }
        })
        .collect()
}

pub fn wifi() -> Option<WifiInfo> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    parse_airport_json(&run("system_profiler", &["SPAirPortDataType", "-json"]))
}

/// When the system started, in seconds since 1970 (`kern.boottime`).
pub fn boot_time() -> i64 {
    #[cfg(target_os = "macos")]
    {
        let mut tv: libc::timeval = libc::timeval { tv_sec: 0, tv_usec: 0 };
        let mut size = std::mem::size_of::<libc::timeval>();
        let name = std::ffi::CString::new("kern.boottime").unwrap();
        // SAFETY: a valid name and an out-buffer of the size given.
        let ok = unsafe { libc::sysctlbyname(name.as_ptr(), &mut tv as *mut _ as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0) } == 0;
        if ok {
            return tv.tv_sec;
        }
        0
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_systems_tools_are_read() {
        let ports = "\nHardware Port: Ethernet\nDevice: en0\nEthernet Address: aa:bb\n\nHardware Port: Wi-Fi\nDevice: en1\nEthernet Address: cc:dd\n\nHardware Port: Thunderbolt Bridge\nDevice: bridge0\n\nHardware Port: iPhone USB\nDevice: en5\n";
        let p = parse_hardware_ports(ports);
        assert_eq!(p.get("en1").map(String::as_str), Some("Wi-Fi"));
        assert_eq!(kind_of_port(&p["en0"]), Kind::Ethernet);
        assert_eq!(kind_of_port(&p["en1"]), Kind::Wifi);
        assert_eq!(kind_of_port(&p["bridge0"]), Kind::Other);
        assert_eq!(kind_of_port(&p["en5"]), Kind::Cellular);
        assert_eq!(kind_of_port("USB 10/100/1000 LAN"), Kind::Ethernet);
        assert_eq!(parse_route_get("   route to: default\ndestination: default\n  gateway: 192.168.1.1\n  interface: en1\n").as_deref(), Some("en1"));
        assert_eq!(parse_route_get("route: writing to routing socket: not in table"), None);
        assert_eq!(parse_media_mbps("en7: flags=8863\n\tmedia: autoselect (1000baseT <full-duplex>)\n\tstatus: active"), Some(1000));
        assert_eq!(parse_media_mbps("\tmedia: autoselect (10Gbase-T <full-duplex>)"), Some(10000));
        assert_eq!(parse_media_mbps("\tmedia: autoselect (2500Base-T <full-duplex>)"), Some(2500));
        assert_eq!(parse_media_mbps("\tmedia: autoselect\n"), None);
        let json = r#"{"SPAirPortDataType":[{"spairport_airport_interfaces":[{"_name":"en1","spairport_current_network_information":{"_name":"Home","spairport_network_rate":866,"spairport_signal_noise":"-55 dBm / -92 dBm"}},{"_name":"awdl0"}]}]}"#;
        let w = parse_airport_json(json).unwrap();
        assert_eq!((w.ssid.as_deref(), w.rssi_dbm, w.signal, w.rx_mbps), (Some("Home"), Some(-55), Some(90), Some(866.0)));
        let hidden = parse_airport_json(&json.replace("\"Home\"", "\"<redacted>\"")).unwrap();
        assert!(hidden.ssid.is_none() && hidden.fix.as_deref() == Some("location"));
        assert!(parse_airport_json(r#"{"SPAirPortDataType":[{"spairport_airport_interfaces":[{"_name":"en1"}]}]}"#).is_none(), "not connected");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_macs_counters_are_read_and_only_grow() {
        let a = counters();
        assert!(a.iter().any(|(n, _, _, _)| n == "lo0"), "{a:?}");
        std::thread::sleep(std::time::Duration::from_millis(200));
        let b = counters();
        for (n, rx, tx, _) in &a {
            if let Some((_, rx2, tx2, _)) = b.iter().find(|(m, _, _, _)| m == n) {
                assert!(rx2 >= rx && tx2 >= tx, "{n} went backwards");
            }
        }
        assert!(boot_time() > 1_600_000_000);
    }
}
