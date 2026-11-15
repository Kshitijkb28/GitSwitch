//! Windows: every adapter's byte counters (`GetIfTable2`, 64-bit), which one
//! carries the traffic now (`GetBestInterfaceEx`: a route lookup, nothing is
//! sent), and the Wi-Fi connection (`WlanQueryInterface`). Since Windows 11
//! 24H2 a desktop app sees the Wi-Fi's name only when location is on for
//! desktop apps; without it, that is said and the setting offered.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use super::{Iface, Kind, WifiInfo};

/// IANA ifType: 71 is Wi-Fi (IEEE 802.11), 6 Ethernet, 243/244 mobile broadband.
/// What isn't the computer's own hardware (a VPN, Hyper-V, Wi-Fi Direct) is virtual.
pub(crate) fn kind_of(if_type: u32, hardware: bool, bluetooth: bool) -> Kind {
    if !hardware {
        return Kind::Virtual;
    }
    if bluetooth {
        return Kind::Other;
    }
    match if_type {
        71 => Kind::Wifi,
        6 => Kind::Ethernet,
        243 | 244 => Kind::Cellular,
        _ => Kind::Other,
    }
}

/// A Wi-Fi's signal as a percentage, from its quality (Windows' 0–100).
pub(crate) fn quality_rssi(quality: u32) -> i32 {
    // Windows maps -100 dBm .. -50 dBm onto 0 .. 100.
    (quality.min(100) as i32) / 2 - 100
}

pub fn read() -> Vec<Iface> {
    #[cfg(windows)]
    {
        sys::read()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

pub fn wifi() -> Option<WifiInfo> {
    #[cfg(windows)]
    {
        sys::wifi()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// When Windows started, in seconds since 1970.
pub fn boot_time() -> i64 {
    #[cfg(windows)]
    {
        // SAFETY: no arguments.
        let up = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() } / 1000;
        chrono::Utc::now().timestamp() - up as i64
    }
    #[cfg(not(windows))]
    {
        0
    }
}

#[cfg(windows)]
mod sys {
    use super::*;
    use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetBestInterfaceEx, GetIfTable2, MIB_IF_TABLE2};
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6};

    /// NdisPhysicalMediumBluetooth.
    const MEDIUM_BLUETOOTH: i32 = 10;

    fn utf16(buf: &[u16]) -> String {
        let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n]).trim().to_string()
    }

    /// The adapter Windows would send internet traffic through now: the route
    /// to 1.1.1.1, or on an IPv6-only network the one to 2606:4700:4700::1111.
    /// A route lookup only; nothing is sent.
    fn best_interface() -> Option<u32> {
        let mut index = 0u32;
        // SAFETY: zeroed socket addresses filled for the two addresses; the out-pointer is valid.
        unsafe {
            let mut addr: SOCKADDR_IN = std::mem::zeroed();
            addr.sin_family = AF_INET;
            addr.sin_addr.S_un.S_addr = u32::from_ne_bytes([1, 1, 1, 1]);
            if GetBestInterfaceEx(&addr as *const SOCKADDR_IN as *const SOCKADDR, &mut index) == 0 {
                return Some(index);
            }
            let mut addr6: SOCKADDR_IN6 = std::mem::zeroed();
            addr6.sin6_family = AF_INET6;
            addr6.sin6_addr.u.Byte = [0x26, 0x06, 0x47, 0x00, 0x47, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0x11, 0x11];
            (GetBestInterfaceEx(&addr6 as *const SOCKADDR_IN6 as *const SOCKADDR, &mut index) == 0).then_some(index)
        }
    }

    pub fn read() -> Vec<Iface> {
        let in_use = best_interface();
        let mut out = Vec::new();
        // SAFETY: the table Windows allocates is read within its entry count
        // and freed with FreeMibTable.
        unsafe {
            let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
            if GetIfTable2(&mut table) != 0 || table.is_null() {
                return out;
            }
            let n = (*table).NumEntries as usize;
            let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), n);
            for r in rows {
                let flags = r.InterfaceAndOperStatusFlags._bitfield;
                let (hardware, filter) = (flags & 0x1 != 0, flags & 0x2 != 0);
                // Filter drivers repeat their adapter's counters; loopback is not traffic.
                if filter || r.Type == 24 {
                    continue;
                }
                let kind = kind_of(r.Type, hardware, r.PhysicalMediumType == MEDIUM_BLUETOOTH);
                let up = r.OperStatus == IfOperStatusUp;
                // Windows keeps many hidden virtual adapters (WAN miniports, Teredo…): only those in use count.
                if kind == Kind::Virtual && !up {
                    continue;
                }
                let name = utf16(&r.Alias);
                if name.is_empty() {
                    continue;
                }
                let link = r.ReceiveLinkSpeed.max(r.TransmitLinkSpeed);
                out.push(Iface {
                    id: format!("{:x}", r.InterfaceLuid.Value),
                    name,
                    detail: Some(utf16(&r.Description)).filter(|d| !d.is_empty()),
                    kind,
                    up,
                    rx: r.InOctets,
                    tx: r.OutOctets,
                    link_bps: (up && link > 0 && link != u64::MAX).then_some(link),
                    in_use: up && Some(r.InterfaceIndex) == in_use,
                });
            }
            FreeMibTable(table as *const _);
        }
        out
    }

    pub fn wifi() -> Option<WifiInfo> {
        use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, HANDLE};
        use windows_sys::Win32::NetworkManagement::WiFi::{
            wlan_interface_state_connected, wlan_intf_opcode_current_connection, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, WLAN_CONNECTION_ATTRIBUTES,
            WLAN_INTERFACE_INFO_LIST,
        };
        // SAFETY: handles and buffers come from the WLAN API and go back to it.
        unsafe {
            let mut version = 0u32;
            let mut h: HANDLE = std::ptr::null_mut();
            if WlanOpenHandle(2, std::ptr::null(), &mut version, &mut h) != 0 {
                return None;
            }
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            let mut info = None;
            if WlanEnumInterfaces(h, std::ptr::null(), &mut list) == 0 && !list.is_null() {
                let n = (*list).dwNumberOfItems as usize;
                let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
                for i in items.iter().filter(|i| i.isState == wlan_interface_state_connected) {
                    let (mut size, mut data, mut kind) = (0u32, std::ptr::null_mut(), 0);
                    let r = WlanQueryInterface(h, &i.InterfaceGuid, wlan_intf_opcode_current_connection, std::ptr::null(), &mut size, &mut data, &mut kind);
                    if r == ERROR_ACCESS_DENIED {
                        info = Some(WifiInfo { note: Some(LOCATION_NOTE.into()), fix: Some("location".into()), ..Default::default() });
                        break;
                    }
                    if r != 0 || data.is_null() {
                        continue;
                    }
                    let a = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let s = &a.wlanAssociationAttributes;
                    let len = (s.dot11Ssid.uSSIDLength as usize).min(32);
                    let ssid = String::from_utf8_lossy(&s.dot11Ssid.ucSSID[..len]).to_string();
                    let quality = s.wlanSignalQuality;
                    let mut w = WifiInfo {
                        ssid: (!ssid.is_empty()).then_some(ssid),
                        signal: Some(quality.min(100)),
                        rssi_dbm: Some(quality_rssi(quality)),
                        rx_mbps: (s.ulRxRate > 0).then(|| s.ulRxRate as f64 / 1000.0),
                        tx_mbps: (s.ulTxRate > 0).then(|| s.ulTxRate as f64 / 1000.0),
                        ..Default::default()
                    };
                    if w.ssid.is_none() {
                        w.note = Some(LOCATION_NOTE.into());
                        w.fix = Some("location".into());
                    }
                    WlanFreeMemory(data as *const _);
                    info = Some(w);
                    break;
                }
            }
            if !list.is_null() {
                WlanFreeMemory(list as *const _);
            }
            WlanCloseHandle(h, std::ptr::null());
            info
        }
    }

    const LOCATION_NOTE: &str = "Windows shows the Wi-Fi's name to apps only when \"Let desktop apps access your location\" is on.";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapters_are_told_apart_by_their_type_and_whether_they_are_hardware() {
        assert_eq!(kind_of(71, true, false), Kind::Wifi);
        assert_eq!(kind_of(6, true, false), Kind::Ethernet);
        assert_eq!(kind_of(243, true, false), Kind::Cellular);
        assert_eq!(kind_of(6, true, true), Kind::Other, "Bluetooth networking");
        assert_eq!(kind_of(6, false, false), Kind::Virtual, "Hyper-V's vEthernet, a VPN");
        assert_eq!(kind_of(71, false, false), Kind::Virtual, "Wi-Fi Direct's virtual adapter");
        assert_eq!(quality_rssi(100), -50);
        assert_eq!(quality_rssi(0), -100);
    }

    #[cfg(windows)]
    #[test]
    fn this_pcs_adapters_are_read_and_their_counters_only_grow() {
        let a = read();
        assert!(!a.is_empty(), "every PC has an adapter");
        std::thread::sleep(std::time::Duration::from_millis(200));
        let b = read();
        for x in &a {
            if let Some(y) = b.iter().find(|y| y.id == x.id) {
                assert!(y.rx >= x.rx && y.tx >= x.tx, "{} went backwards", x.name);
            }
        }
        assert!(a.iter().filter(|i| i.in_use).count() <= 1);
        assert!(boot_time() > 1_600_000_000 && boot_time() <= chrono::Utc::now().timestamp());
        let _ = wifi();
    }
}
