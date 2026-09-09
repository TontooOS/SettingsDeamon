//! WiFi backend owned by the settings daemon.
//!
//! The daemon is the only WiFi writer: it drives NetworkManager through
//! NetworkKit's direct reads and persists known networks as `KnownWifi`
//! entities in CoreData under the daemon bundle id.
//!
//! Public read ops (`wifi_list`, `wifi_status`) are served to every client
//! (NetworkKit queries them). Write ops (`wifi_connect`, `wifi_disconnect`,
//! `wifi_enable`, `wifi_disable`, `wifi_forget`) have no public client
//! library and are reserved for the Settings app (`com.tontoo.systemsettings`).

use std::collections::HashSet;

use coredata::{PersistentContainer, StoreType};
use networkkit::util;
use networkkit::wifi::{Wifi, WifiNetwork, WifiStatus};

/// Bundle id owning the known-network store.
pub const DAEMON_BUNDLE_ID: &str = "com.tontoo.settingsdaemon";
/// CoreData entity holding one known network per object.
pub const KNOWN_WIFI_ENTITY: &str = "KnownWifi";

pub const OP_WIFI_LIST: &str = "wifi_list";
pub const OP_WIFI_STATUS: &str = "wifi_status";
pub const OP_WIFI_CONNECT: &str = "wifi_connect";
pub const OP_WIFI_DISCONNECT: &str = "wifi_disconnect";
pub const OP_WIFI_ENABLE: &str = "wifi_enable";
pub const OP_WIFI_DISABLE: &str = "wifi_disable";
pub const OP_WIFI_FORGET: &str = "wifi_forget";

/// Scan nearby networks, flagging stored known networks.
/// Unknown networks carry `known: false`, known ones `known: true`.
pub fn list() -> Result<Vec<WifiNetwork>, String> {
    let known = known_ssids().unwrap_or_default();
    let networks = Wifi::new()
        .scan_direct(true)
        .map_err(|e| e.to_string())?;
    Ok(mark_known(networks, &known))
}

/// Set the `known` flag on scan results (pure helper, unit-tested).
pub fn mark_known(mut networks: Vec<WifiNetwork>, known: &HashSet<String>) -> Vec<WifiNetwork> {
    for network in &mut networks {
        network.known = known.contains(&network.ssid);
    }
    networks
}

/// Radio state plus the current connection, if any.
/// Returns `{"enabled": bool, "status": WifiStatus|null}`.
pub fn status() -> Result<serde_json::Value, String> {
    let enabled = radio_enabled()?;
    let current = Wifi::new().status_direct().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"enabled": enabled, "status": current}))
}

/// Connect to a network (open networks take `password: None`).
/// On success the network is stored as known.
pub fn connect(
    ssid: &str,
    password: Option<&str>,
    hidden: bool,
) -> Result<WifiStatus, String> {
    if ssid.is_empty() {
        return Err("ssid must not be empty".to_string());
    }
    let wifi = Wifi::new();
    if !wifi.is_available() {
        return Err("wifi not available".to_string());
    }

    let mut args: Vec<String> = vec![
        "--terse".into(),
        "device".into(),
        "wifi".into(),
        "connect".into(),
        ssid.into(),
    ];
    if let Some(password) = password {
        args.push("password".into());
        args.push(password.into());
    }
    if hidden {
        args.push("hidden".into());
        args.push("yes".into());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    util::run("nmcli", &refs).map_err(|e| e.to_string())?;

    let current = wifi.status_direct().map_err(|e| e.to_string())?;
    match current {
        Some(status) if status.ssid.as_deref() == Some(ssid) => {
            remember_known(ssid, &status.security)?;
            Ok(status)
        }
        _ => Err(format!("could not associate with {}", ssid)),
    }
}

/// Disconnect the wireless interface.
pub fn disconnect() -> Result<(), String> {
    let wifi = Wifi::new();
    let iface = wifi.interface().ok_or_else(|| "wifi not available".to_string())?;
    util::run("nmcli", &["device", "disconnect", &iface]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Switch the WLAN radio on or off (`nmcli radio wifi on|off`).
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let arg = if enabled { "on" } else { "off" };
    util::run("nmcli", &["radio", "wifi", arg]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a known network: removes the NetworkManager profile
/// (best effort) and the CoreData entry. Returns true when anything
/// was removed.
pub fn forget(ssid: &str) -> Result<bool, String> {
    if ssid.is_empty() {
        return Err("ssid must not be empty".to_string());
    }
    let profile_removed = util::run("nmcli", &["connection", "delete", ssid]).is_ok();
    let store_removed = delete_known(ssid)?;
    Ok(profile_removed || store_removed)
}

/// True when `nmcli radio wifi` reports `enabled`.
fn radio_enabled() -> Result<bool, String> {
    let out = util::run("nmcli", &["radio", "wifi"]).map_err(|e| e.to_string())?;
    Ok(out.trim().eq_ignore_ascii_case("enabled"))
}

// ---------------------------------------------------------------------------
// Known networks in CoreData
// ---------------------------------------------------------------------------

fn container() -> Result<PersistentContainer, String> {
    PersistentContainer::new_with_bundle(DAEMON_BUNDLE_ID.to_string(), StoreType::Fico)
        .map_err(|e| e.to_string())
}

/// SSIDs stored as known networks.
pub fn known_ssids() -> Result<HashSet<String>, String> {
    let mut store = container()?;
    let ctx = store.view_context();
    let objects = ctx.fetch_all(KNOWN_WIFI_ENTITY).map_err(|e| e.to_string())?;
    Ok(objects
        .iter()
        .filter_map(|o| o.get_str("ssid").map(str::to_string))
        .collect())
}

/// Insert or refresh the known-network entry after a successful connect.
fn remember_known(ssid: &str, security: &str) -> Result<(), String> {
    let mut store = container()?;
    let now = unix_now();
    {
        let mut ctx = store.view_context();
        let mut objects = ctx.fetch_all(KNOWN_WIFI_ENTITY).map_err(|e| e.to_string())?;
        if let Some(existing) = objects.iter_mut().find(|o| o.get_str("ssid") == Some(ssid)) {
            existing.set("security", security.to_string());
            existing.set("last_connected", now);
            let updated = existing.clone();
            ctx.save_object(updated).map_err(|e| e.to_string())?;
        } else {
            let mut obj = ctx.create(KNOWN_WIFI_ENTITY);
            obj.set("ssid", ssid.to_string());
            obj.set("security", security.to_string());
            obj.set("last_connected", now);
            obj.set("auto_join", true);
            ctx.save_object(obj).map_err(|e| e.to_string())?;
        }
        ctx.save().map_err(|e| e.to_string())?;
    }
    store.save().map_err(|e| e.to_string())
}

/// Delete the known-network entry. Returns true when one existed.
fn delete_known(ssid: &str) -> Result<bool, String> {
    let mut store = container()?;
    let removed = {
        let mut ctx = store.view_context();
        let objects = ctx.fetch_all(KNOWN_WIFI_ENTITY).map_err(|e| e.to_string())?;
        let ids: Vec<String> = objects
            .iter()
            .filter(|o| o.get_str("ssid") == Some(ssid))
            .map(|o| o.object_id.clone())
            .collect();
        for id in &ids {
            ctx.delete(id).map_err(|e| e.to_string())?;
        }
        if !ids.is_empty() {
            ctx.save().map_err(|e| e.to_string())?;
        }
        !ids.is_empty()
    };
    store.save().map_err(|e| e.to_string())?;
    Ok(removed)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<WifiNetwork> {
        vec![
            WifiNetwork {
                ssid: "HomeNet".into(),
                bssid: None,
                signal_pct: 80,
                frequency_mhz: Some(2437),
                security: "WPA2".into(),
                known: false,
            },
            WifiNetwork {
                ssid: "Cafe".into(),
                bssid: None,
                signal_pct: 40,
                frequency_mhz: Some(5180),
                security: "OPEN".into(),
                known: false,
            },
        ]
    }

    #[test]
    fn marks_only_stored_ssids_as_known() {
        let known: HashSet<String> = ["HomeNet".to_string()].into_iter().collect();
        let marked = mark_known(sample(), &known);
        assert!(marked[0].known);
        assert!(!marked[1].known);
    }

    #[test]
    fn empty_store_marks_everything_unknown() {
        let marked = mark_known(sample(), &HashSet::new());
        assert!(marked.iter().all(|n| !n.known));
    }

    #[test]
    fn rejects_empty_ssid() {
        assert!(connect("", None, false).is_err());
        assert!(forget("").is_err());
    }
}
