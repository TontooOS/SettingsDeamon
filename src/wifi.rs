//! WiFi backend owned by the settings daemon.
//!
//! The daemon is the only WiFi writer: it drives NetworkManager through
//! NetworkKit's direct reads and persists known networks (including the
//! encrypted password) as `KnownWifi` entities in a system-wide CoreData
//! store at `/System/Preferences/com.tontoo.wifi/storage.fico`, shared by
//! all users.
//!
//! Public read ops (`wifi_list`, `wifi_status`, `wifi_known_list`) are
//! served to every client (NetworkKit queries them). `wifi_known_list`
//! never exposes passwords. Write ops (`wifi_connect`, `wifi_disconnect`,
//! `wifi_enable`, `wifi_disable`, `wifi_forget`) have no public client
//! library and are reserved for the Settings app (`com.tontoo.systemsettings`).
//! On startup the daemon auto-joins the most recently used visible known
//! network (`auto_join`).

use std::collections::HashSet;

use coredata::{PersistentContainer, StoreType};
use networkkit::util;
use networkkit::wifi::{Wifi, WifiNetwork, WifiStatus};
use serde::Serialize;

/// Bundle id owning the per-user daemon store (wallpaper customs, ...).
pub const DAEMON_BUNDLE_ID: &str = "com.tontoo.settingsdaemon";
/// Bundle id owning the system-wide known-network store.
pub const SYSTEM_WIFI_BUNDLE_ID: &str = "com.tontoo.wifi";
/// CoreData entity holding one known network per object.
pub const KNOWN_WIFI_ENTITY: &str = "KnownWifi";

pub const OP_WIFI_LIST: &str = "wifi_list";
pub const OP_WIFI_STATUS: &str = "wifi_status";
pub const OP_WIFI_KNOWN_LIST: &str = "wifi_known_list";
pub const OP_WIFI_CONNECT: &str = "wifi_connect";
pub const OP_WIFI_DISCONNECT: &str = "wifi_disconnect";
pub const OP_WIFI_ENABLE: &str = "wifi_enable";
pub const OP_WIFI_DISABLE: &str = "wifi_disable";
pub const OP_WIFI_FORGET: &str = "wifi_forget";

/// One stored known network (passwords are never exposed through this type).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnownNetwork {
    pub ssid: String,
    pub security: String,
    pub last_connected: i64,
    pub auto_join: bool,
}

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
/// Returns `{"enabled": bool, "status": WifiStatus|null, "available": bool}`.
/// `available` is false when no wireless adapter exists; then `enabled` is
/// false and `status` is null instead of an error, so clients can show the
/// no-hardware message.
pub fn status() -> Result<serde_json::Value, String> {
    let wifi = Wifi::new();
    let available = wifi.is_available();
    let (enabled, current) = if available {
        let enabled = radio_enabled()?;
        let current = wifi.status_direct().map_err(|e| e.to_string())?;
        (enabled, current)
    } else {
        (false, None)
    };
    Ok(serde_json::json!({"enabled": enabled, "status": current, "available": available}))
}

/// Stored known networks, most recently connected first (no passwords).
pub fn known_list() -> Result<Vec<KnownNetwork>, String> {
    known_networks()
}

/// Connect to a network (open networks take `password: None`).
/// On success the network (including its password) is stored as known.
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

    let current = connect_inner(&wifi, ssid, password, hidden)?;
    match current {
        Some(status) if status.ssid.as_deref() == Some(ssid) => {
            remember_known(ssid, &status.security, password)?;
            Ok(status)
        }
        _ => Err(format!("could not associate with {}", ssid)),
    }
}

/// Run the `nmcli device wifi connect` call and return the verified
/// connection status (no store writes; shared by `connect` and `auto_join`).
fn connect_inner(
    wifi: &Wifi,
    ssid: &str,
    password: Option<&str>,
    hidden: bool,
) -> Result<Option<WifiStatus>, String> {
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

    wifi.status_direct().map_err(|e| e.to_string())
}

/// Auto-join at daemon startup: when a wireless adapter exists, the radio
/// is on and nothing is connected yet, join the most recently used visible
/// known network with `auto_join` set. Returns the connection, if any.
/// Never fails the startup: every degraded state yields `Ok(None)`.
pub fn auto_join() -> Result<Option<WifiStatus>, String> {
    let wifi = Wifi::new();
    if !wifi.is_available() {
        return Ok(None);
    }
    let current = wifi.status_direct().map_err(|e| e.to_string())?;
    if current.as_ref().and_then(|s| s.ssid.as_deref()).is_some() {
        return Ok(current);
    }
    if !radio_enabled().unwrap_or(false) {
        return Ok(None);
    }
    let visible: HashSet<String> = wifi
        .scan_direct(false)
        .map(|networks| networks.into_iter().map(|n| n.ssid).collect())
        .unwrap_or_default();
    for entry in known_networks()?.into_iter().filter(|e| e.auto_join) {
        if !visible.contains(&entry.ssid) {
            continue;
        }
        let password = known_password(&entry.ssid)?;
        let password_ref = password.as_deref().filter(|p| !p.is_empty());
        match connect_inner(&wifi, &entry.ssid, password_ref, false) {
            Ok(Some(status)) if status.ssid.as_deref() == Some(entry.ssid.as_str()) => {
                remember_known(&entry.ssid, &status.security, password_ref)?;
                return Ok(Some(status));
            }
            _ => continue,
        }
    }
    Ok(None)
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
// Known networks in CoreData (system-wide store)
// ---------------------------------------------------------------------------

fn container() -> Result<PersistentContainer, String> {
    PersistentContainer::new_system_with_bundle(SYSTEM_WIFI_BUNDLE_ID.to_string(), StoreType::Fico)
        .map_err(|e| e.to_string())
}

/// SSIDs stored as known networks.
pub fn known_ssids() -> Result<HashSet<String>, String> {
    Ok(known_networks()?
        .into_iter()
        .map(|n| n.ssid)
        .collect())
}

/// Stored known networks, most recently connected first.
pub fn known_networks() -> Result<Vec<KnownNetwork>, String> {
    let mut store = container()?;
    let ctx = store.view_context();
    let objects = ctx.fetch_all(KNOWN_WIFI_ENTITY).map_err(|e| e.to_string())?;
    let mut out: Vec<KnownNetwork> = objects
        .iter()
        .filter_map(|o| {
            Some(KnownNetwork {
                ssid: o.get_str("ssid")?.to_string(),
                security: o.get_str("security").unwrap_or("").to_string(),
                last_connected: o.get_i64("last_connected").unwrap_or(0),
                auto_join: o.get_bool("auto_join").unwrap_or(true),
            })
        })
        .collect();
    out.sort_by(|a, b| b.last_connected.cmp(&a.last_connected));
    Ok(out)
}

/// Stored password for one SSID, if any (open networks store none).
fn known_password(ssid: &str) -> Result<Option<String>, String> {
    let mut store = container()?;
    let ctx = store.view_context();
    let objects = ctx.fetch_all(KNOWN_WIFI_ENTITY).map_err(|e| e.to_string())?;
    Ok(objects
        .iter()
        .find(|o| o.get_str("ssid") == Some(ssid))
        .and_then(|o| o.get_str("password"))
        .filter(|p| !p.is_empty())
        .map(str::to_string))
}

/// Insert or refresh the known-network entry after a successful connect.
/// The password is stored encrypted alongside the SSID so the daemon can
/// auto-join at startup; open networks store no password.
fn remember_known(ssid: &str, security: &str, password: Option<&str>) -> Result<(), String> {
    let mut store = container()?;
    let now = unix_now();
    {
        let mut ctx = store.view_context();
        let mut objects = ctx.fetch_all(KNOWN_WIFI_ENTITY).map_err(|e| e.to_string())?;
        if let Some(existing) = objects.iter_mut().find(|o| o.get_str("ssid") == Some(ssid)) {
            existing.set("security", security.to_string());
            existing.set("last_connected", now);
            if let Some(password) = password {
                existing.set("password", password.to_string());
            }
            let updated = existing.clone();
            ctx.save_object(updated).map_err(|e| e.to_string())?;
        } else {
            let mut obj = ctx.create(KNOWN_WIFI_ENTITY);
            obj.set("ssid", ssid.to_string());
            obj.set("security", security.to_string());
            obj.set("last_connected", now);
            obj.set("auto_join", true);
            if let Some(password) = password {
                obj.set("password", password.to_string());
            }
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

    static STORE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Point the system store at a scratch dir (env process-global, so the
    /// module lock serializes these tests). Returns the scratch dir.
    fn with_scratch_system_store(tag: &str) -> (std::path::PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = STORE_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "settingsdaemon-wifi-{}-{}.d",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TONTOO_SYSTEM_PREFERENCES_ROOT", &dir);
        std::env::set_var("TONTOO_COREDATA_ALLOW_FOREIGN", "1");
        (dir, guard)
    }

    #[test]
    fn known_store_roundtrip_keeps_password_out_of_list() {
        let (dir, _guard) = with_scratch_system_store("roundtrip");
        assert!(known_networks().unwrap().is_empty());

        remember_known("HomeNet", "WPA2", Some("s3cret")).unwrap();
        remember_known("Cafe", "OPEN", None).unwrap();

        let known = known_networks().unwrap();
        assert_eq!(known.len(), 2);
        let home = known.iter().find(|n| n.ssid == "HomeNet").unwrap();
        assert_eq!(home.security, "WPA2");
        assert!(home.auto_join);
        assert_eq!(known_password("HomeNet").unwrap().as_deref(), Some("s3cret"));
        assert_eq!(known_password("Cafe").unwrap(), None);

        // Refreshing updates in place instead of duplicating.
        remember_known("HomeNet", "WPA3", Some("n3w")).unwrap();
        let known = known_networks().unwrap();
        assert_eq!(known.len(), 2);
        assert_eq!(
            known.iter().find(|n| n.ssid == "HomeNet").unwrap().security,
            "WPA3"
        );
        assert_eq!(known_password("HomeNet").unwrap().as_deref(), Some("n3w"));

        // The public list shape serializes without any password material.
        let listed = known_list().unwrap();
        let json = serde_json::to_value(&listed).unwrap();
        assert!(!json.to_string().contains("s3cret"));
        assert!(!json.to_string().contains("n3w"));
        assert!(!json.to_string().contains("password"));

        assert!(known_ssids().unwrap().contains("HomeNet"));
        assert!(forget("HomeNet").unwrap());
        assert!(!forget("HomeNet").unwrap());
        assert_eq!(known_networks().unwrap().len(), 1);
        assert!(forget("Cafe").unwrap());
        assert!(known_networks().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_join_without_adapter_returns_none() {
        let (_dir, _guard) = with_scratch_system_store("autojoin");
        // The mandated test environment (WSL ArchLinux) has no wireless
        // adapter, so auto-join degrades to Ok(None) without touching nmcli.
        if !Wifi::new().is_available() {
            assert!(auto_join().unwrap().is_none());
        }
    }
}
