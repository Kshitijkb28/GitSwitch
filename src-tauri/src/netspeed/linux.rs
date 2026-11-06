//! Linux: `/proc/net/dev` for the counters, `/sys/class/net` for what each
//! adapter is, the default route for the one in use, and NetworkManager
//! (`nmcli`) for the Wi-Fi — `/proc/net/wireless` when it isn't there.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use super::{Iface, Kind, WifiInfo};
use std::path::Path;

/// `/proc/net/dev` → (adapter, received bytes, sent bytes).
pub(crate) fn parse_proc_net_dev(text: &str) -> Vec<(String, u64, u64)> {
    text.lines()
        .skip(2)
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let f: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (f.len() >= 9).then(|| (name.trim().to_string(), f[0], f[8]))
        })
        .collect()
}

/// `/proc/net/route` → the adapter of the default route with the lowest metric.
pub(crate) fn parse_default_route(text: &str) -> Option<String> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 8 && f[1] == "00000000" && f[7] == "00000000").then(|| (f[0].to_string(), f[6].parse::<u32>().unwrap_or(u32::MAX)))
        })
        .min_by_key(|(_, metric)| *metric)
        .map(|(name, _)| name)
}

/// One `nmcli -t` line's fields: `:` separates them, `\:` is a colon inside one.
fn nmcli_fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.last_mut().unwrap().push(n);
                }
            }
            ':' => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out
}

/// `nmcli -t -f ACTIVE,SSID,SIGNAL,RATE,DEVICE dev wifi` → the connected network.
pub(crate) fn parse_nmcli_wifi(text: &str) -> Option<(WifiInfo, String)> {
    for l in text.lines() {
        let f = nmcli_fields(l);
        if f.len() < 5 || f[0] != "yes" {
            continue;
        }
        let rate = f[3].split_whitespace().next().and_then(|r| r.parse::<f64>().ok());
        let signal = f[2].trim().parse::<u32>().ok();
        let info = WifiInfo { ssid: Some(f[1].clone()).filter(|s| !s.is_empty()), signal, rssi_dbm: None, rx_mbps: rate, tx_mbps: None, note: None, fix: None };
        return Some((info, f[4].clone()));
    }
    None
}

/// `/proc/net/wireless` → the adapter's link quality as a percentage (of 70).
pub(crate) fn parse_proc_net_wireless(text: &str, dev: &str) -> Option<u32> {
    for l in text.lines().skip(2) {
        let (name, rest) = l.split_once(':')?;
        if name.trim() != dev {
            continue;
        }
        let q: f64 = rest.split_whitespace().nth(1)?.trim_end_matches('.').parse().ok()?;
        return Some(((q / 70.0) * 100.0).round().clamp(0.0, 100.0) as u32);
    }
    None
}

fn kind(name: &str, sys: &Path) -> Kind {
    let dir = sys.join(name);
    if dir.join("wireless").exists() || dir.join("phy80211").exists() {
        Kind::Wifi
    } else if name.starts_with("ww") {
        Kind::Cellular
    } else if dir.join("device").exists() {
        // A physical adapter; type 1 is Ethernet.
        match std::fs::read_to_string(dir.join("type")).ok().map(|t| t.trim().to_string()).as_deref() {
            Some("1") => Kind::Ethernet,
            _ => Kind::Other,
        }
    } else {
        Kind::Virtual
    }
}

pub fn read() -> Vec<Iface> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let Ok(dev) = std::fs::read_to_string("/proc/net/dev") else { return Vec::new() };
    let default = std::fs::read_to_string("/proc/net/route").ok().and_then(|t| parse_default_route(&t));
    let sys = Path::new("/sys/class/net");
    parse_proc_net_dev(&dev)
        .into_iter()
        .filter(|(n, _, _)| n != "lo")
        .map(|(name, rx, tx)| {
            let k = kind(&name, sys);
            let state = std::fs::read_to_string(sys.join(&name).join("operstate")).unwrap_or_default();
            let up = matches!(state.trim(), "up" | "unknown");
            let speed = std::fs::read_to_string(sys.join(&name).join("speed")).ok().and_then(|s| s.trim().parse::<i64>().ok()).filter(|s| *s > 0);
            Iface {
                id: name.clone(),
                in_use: up && default.as_deref() == Some(name.as_str()),
                detail: None,
                kind: k,
                up,
                rx,
                tx,
                link_bps: if k == Kind::Wifi { None } else { speed.map(|m| m as u64 * 1_000_000) },
                name,
            }
        })
        .collect()
}

pub fn wifi() -> Option<WifiInfo> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    if let Ok(o) = crate::spawn::command("nmcli").args(["-t", "-f", "ACTIVE,SSID,SIGNAL,RATE,DEVICE", "dev", "wifi"]).output() {
        if let Some((info, _)) = parse_nmcli_wifi(&String::from_utf8_lossy(&o.stdout)) {
            return Some(info);
        }
    }
    let wireless = std::fs::read_to_string("/proc/net/wireless").ok()?;
    let dev = read().into_iter().find(|i| i.kind == Kind::Wifi && i.up)?.name;
    let signal = parse_proc_net_wireless(&wireless, &dev)?;
    Some(WifiInfo { signal: Some(signal), note: Some("The Wi-Fi's name needs NetworkManager (nmcli).".into()), ..Default::default() })
}

/// When the system started, in seconds since 1970 (`btime` in /proc/stat).
pub fn boot_time() -> i64 {
    std::fs::read_to_string("/proc/stat").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("btime ").and_then(|v| v.trim().parse().ok()))).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernels_tables_are_read() {
        let dev = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 1234 10 0 0 0 0 0 0 1234 10 0 0 0 0 0 0\nwlp2s0: 987654321 800000 0 0 0 0 0 12 123456789 400000 0 0 0 0 0 0\n";
        assert_eq!(parse_proc_net_dev(dev), vec![("lo".into(), 1234, 1234), ("wlp2s0".into(), 987654321, 123456789)]);
        let route = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\neth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\nwlp2s0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\nwlp2s0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n";
        assert_eq!(parse_default_route(route).as_deref(), Some("eth0"), "the lowest metric");
        assert_eq!(parse_default_route("Iface\tDestination\n"), None);
        let nm = "no:Neighbour:30:54 Mbit/s:wlp2s0\nyes:Home\\:5G:72:540 Mbit/s:wlp2s0\n";
        let (w, dev) = parse_nmcli_wifi(nm).unwrap();
        assert_eq!((w.ssid.as_deref(), w.signal, w.rx_mbps, dev.as_str()), (Some("Home:5G"), Some(72), Some(540.0), "wlp2s0"));
        assert!(parse_nmcli_wifi("no:x:1:1 Mbit/s:wlan0\n").is_none());
        let wl = "Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE\n face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22\nwlp2s0: 0000   56.  -54.  -256        0      0      0      0     12        0\n";
        assert_eq!(parse_proc_net_wireless(wl, "wlp2s0"), Some(80));
        assert_eq!(parse_proc_net_wireless(wl, "wlan9"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_machines_adapters_are_read_and_their_counters_only_grow() {
        let a = read();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let b = read();
        for x in &a {
            if let Some(y) = b.iter().find(|y| y.id == x.id) {
                assert!(y.rx >= x.rx && y.tx >= x.tx, "{} went backwards", x.name);
            }
        }
        assert!(boot_time() > 1_600_000_000);
    }
}
