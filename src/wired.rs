//! Wired backend owned by the settings daemon.
//!
//! Lists connected Ethernet interfaces with details (`wired_list` public
//! read op). Device enumeration and addresses come from NetworkManager;
//! MAC, MTU, speed and driver come from sysfs. Only connected interfaces
//! are reported.

use networkkit::util;
use serde::Serialize;

pub const OP_WIRED_LIST: &str = "wired_list";

/// One connected wired interface with details for the info menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WiredInfo {
    pub interface: String,
    pub connection: String,
    pub state: String,
    pub ipv4_addrs: Vec<String>,
    pub gateway: Option<String>,
    pub mac: String,
    pub speed_mbps: Option<u32>,
    pub mtu: Option<u32>,
    pub driver: Option<String>,
}

/// Parse one `nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device status`
/// line. Returns `(device, connection)` for connected Ethernet devices.
fn parse_status_line(line: &str) -> Option<(String, String)> {
    let fields = networkkit::wifi::split_terse(line);
    if fields.len() < 4 {
        return None;
    }
    if fields[1] != "ethernet" || fields[2] != "connected" {
        return None;
    }
    if fields[0].is_empty() || fields[3].is_empty() {
        return None;
    }
    Some((fields[0].clone(), fields[3].clone()))
}

/// Parse `nmcli device show` output: IPv4 addresses (without prefix),
/// gateway and state.
fn parse_device_show(output: &str) -> (Vec<String>, Option<String>, String) {
    let mut addrs: Vec<String> = Vec::new();
    let mut gateway: Option<String> = None;
    let mut state = String::new();
    for line in output.lines() {
        if let Some(value) = line.strip_prefix("IP4.ADDRESS") {
            if let Some(addr) = value.split_once(':').map(|(_, v)| v.trim()) {
                let ip = addr.split('/').next().unwrap_or("").trim();
                if !ip.is_empty() && !addrs.iter().any(|a| a == ip) {
                    addrs.push(ip.to_string());
                }
            }
        } else if let Some(value) = line.strip_prefix("IP4.GATEWAY:") {
            let gw = value.trim();
            if !gw.is_empty() && gateway.is_none() {
                gateway = Some(gw.to_string());
            }
        } else if let Some(value) = line.strip_prefix("GENERAL.STATE:") {
            // "100 (connected)" -> "connected"
            let text = value.trim();
            state = text
                .split_once('(')
                .map(|(_, rest)| rest.trim_end_matches(')').trim().to_string())
                .unwrap_or_else(|| text.to_string());
        }
    }
    (addrs, gateway, state)
}

/// Sysfs details for one interface (`sysfs_base` is `/sys/class/net` on
/// real systems, injectable for tests).
fn read_sysfs(sysfs_base: &str, interface: &str) -> (String, Option<u32>, Option<u32>, Option<String>) {
    let base = std::path::Path::new(sysfs_base).join(interface);
    let mac = std::fs::read_to_string(base.join("address"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let speed_mbps = std::fs::read_to_string(base.join("speed"))
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .map(|v| v as u32);
    let mtu = std::fs::read_to_string(base.join("mtu"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    let driver = std::fs::read_link(base.join("device").join("driver"))
        .ok()
        .and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
        });
    (mac, speed_mbps, mtu, driver)
}

/// Connected Ethernet interfaces with details, ordered by interface name.
pub fn list() -> Result<Vec<WiredInfo>, String> {
    let out = util::run(
        "nmcli",
        &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"],
    )
    .map_err(|e| e.to_string())?;
    let mut devices: Vec<(String, String)> = out.lines().filter_map(parse_status_line).collect();
    devices.sort();
    devices.dedup();
    let mut infos = Vec::new();
    for (interface, connection) in devices {
        let show = util::run(
            "nmcli",
            &[
                "-t",
                "-f",
                "GENERAL.STATE,IP4.ADDRESS,IP4.GATEWAY",
                "device",
                "show",
                &interface,
            ],
        )
        .map_err(|e| e.to_string())?;
        let (ipv4_addrs, gateway, state) = parse_device_show(&show);
        let (mac, speed_mbps, mtu, driver) = read_sysfs("/sys/class/net", &interface);
        infos.push(WiredInfo {
            interface,
            connection,
            state,
            ipv4_addrs,
            gateway,
            mac,
            speed_mbps,
            mtu,
            driver,
        });
    }
    Ok(infos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_filters_connected_ethernet() {
        assert_eq!(
            parse_status_line("eth0:ethernet:connected:Wired connection 1"),
            Some(("eth0".to_string(), "Wired connection 1".to_string()))
        );
        assert_eq!(parse_status_line("wlan0:wifi:connected:HomeNet"), None);
        assert_eq!(
            parse_status_line("eth1:ethernet:disconnected:Wired connection 2"),
            None
        );
        assert_eq!(parse_status_line("eth0:ethernet:connected:"), None);
        assert_eq!(parse_status_line("garbage"), None);
    }

    #[test]
    fn device_show_parses_addrs_gateway_state() {
        let out = "GENERAL.STATE:100 (connected)\nIP4.ADDRESS[1]:192.168.1.5/24\nIP4.ADDRESS[2]:10.0.0.5/8\nIP4.GATEWAY:192.168.1.1\nIP4.DNS[1]:1.1.1.1\n";
        let (addrs, gateway, state) = parse_device_show(out);
        assert_eq!(addrs, vec!["192.168.1.5", "10.0.0.5"]);
        assert_eq!(gateway.as_deref(), Some("192.168.1.1"));
        assert_eq!(state, "connected");
    }

    #[test]
    fn device_show_handles_missing_fields() {
        let (addrs, gateway, state) = parse_device_show("GENERAL.STATE:30 (disconnected)\n");
        assert!(addrs.is_empty());
        assert_eq!(gateway, None);
        assert_eq!(state, "disconnected");
    }

    #[test]
    fn sysfs_reads_fake_tree() {
        let dir = std::env::temp_dir().join(format!("wired-sysfs-{}", std::process::id()));
        let iface = dir.join("eth9");
        std::fs::create_dir_all(iface.join("device").join("e1000e")).unwrap();
        std::fs::write(iface.join("address"), "aa:bb:cc:dd:ee:ff\n").unwrap();
        std::fs::write(iface.join("speed"), "1000\n").unwrap();
        std::fs::write(iface.join("mtu"), "1500\n").unwrap();
        std::os::unix::fs::symlink(iface.join("device").join("e1000e"), iface.join("device").join("driver")).unwrap();
        let base = dir.to_string_lossy().to_string();
        let (mac, speed, mtu, driver) = read_sysfs(&base, "eth9");
        assert_eq!(mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(speed, Some(1000));
        assert_eq!(mtu, Some(1500));
        assert_eq!(driver.as_deref(), Some("e1000e"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sysfs_rejects_unknown_speed() {
        let dir = std::env::temp_dir().join(format!("wired-sysfs-neg-{}", std::process::id()));
        let iface = dir.join("eth9");
        std::fs::create_dir_all(&iface).unwrap();
        std::fs::write(iface.join("speed"), "-1\n").unwrap();
        let base = dir.to_string_lossy().to_string();
        let (_, speed, _, _) = read_sysfs(&base, "eth9");
        assert_eq!(speed, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_without_nmcli_errors() {
        if !networkkit::util::tool_available("nmcli") {
            assert!(list().is_err());
        }
    }
}
