//! Date & Time backend owned by the settings daemon.
//!
//! Reads NTP state and timezone via `timedatectl`, applies timezones with
//! `timedatectl set-timezone`, and persists the 24-hour display
//! preference in the `datetime` store domain. `datetime_get` is a public
//! read op; `datetime_set_timezone` and `datetime_set_24h` are private
//! write ops reserved for the Settings app (`com.tontoo.systemsettings`).
//!
//! The timezone list comes from `timedatectl list-timezones` with a
//! `/usr/share/zoneinfo` walk as fallback, so clients always receive a
//! real, non-empty list.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::store::SettingsStore;

/// Store domain holding the clock display preference.
pub const DOMAIN: &str = "datetime";
/// 24-hour display preference key.
pub const KEY_USE_24H: &str = "use_24h";

pub const OP_DATETIME_GET: &str = "datetime_get";
pub const OP_DATETIME_SET_TIMEZONE: &str = "datetime_set_timezone";
pub const OP_DATETIME_SET_24H: &str = "datetime_set_24h";

/// Effective date & time state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateTimeState {
    pub ntp: bool,
    pub timezone: String,
    pub use_24h: bool,
    pub timezones: Vec<String>,
}

/// One `timedatectl show` property value, empty when unavailable.
fn timedatectl_prop(prop: &str) -> String {
    networkkit::util::run("timedatectl", &["show", "-p", prop, "--value"])
        .map(|out| out.trim().to_string())
        .unwrap_or_default()
}

/// Current timezone: `timedatectl`, else the `/etc/localtime` symlink
/// target below `/usr/share/zoneinfo`, else `UTC`.
fn current_timezone() -> String {
    let prop = timedatectl_prop("Timezone");
    if !prop.is_empty() {
        return prop;
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let text = target.to_string_lossy().replace('\\', "/");
        if let Some(zone) = text.split("/zoneinfo/").last() {
            if !zone.is_empty() && !zone.contains("..") {
                return zone.to_string();
            }
        }
    }
    "UTC".to_string()
}

/// NTP sync state from `timedatectl` (false when unavailable).
fn ntp_active() -> bool {
    timedatectl_prop("NTP").eq_ignore_ascii_case("yes")
}

/// Ensure automatic time sync stays on: enables NTP and returns the
/// active state afterwards. Best effort for daemon startup; callers log
/// the error instead of failing.
pub fn ensure_ntp() -> Result<bool, String> {
    networkkit::util::run("timedatectl", &["set-ntp", "true"])
        .map_err(|e| format!("datetime ntp failed: {}", e))?;
    Ok(ntp_active())
}

/// Non-zone files below `/usr/share/zoneinfo` (tables, POSIX variants).
fn is_zone_file(relative: &str) -> bool {
    if relative.is_empty()
        || relative.starts_with('.')
        || relative.contains("..")
        || relative == "zone.tab"
        || relative == "zone1970.tab"
        || relative == "iso3166.tab"
        || relative == "leap-seconds.list"
        || relative == "tzdata.zi"
        || relative == "leapseconds"
        || relative.starts_with("posix/")
        || relative.starts_with("right/")
    {
        return false;
    }
    true
}

fn walk_zoneinfo(base: &std::path::Path, prefix: &str, out: &mut Vec<String>) {
    let entries = match std::fs::read_dir(if prefix.is_empty() {
        base.to_path_buf()
    } else {
        base.join(prefix)
    }) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", prefix, name)
        };
        let path = base.join(&relative);
        if path.is_dir() {
            walk_zoneinfo(base, &relative, out);
        } else if is_zone_file(&relative) {
            out.push(relative);
        }
    }
}

/// Real timezone list: `timedatectl list-timezones`, else a
/// `/usr/share/zoneinfo` walk, else `UTC` alone. Never empty.
pub fn timezone_list() -> Vec<String> {
    if let Ok(out) = networkkit::util::run("timedatectl", &["list-timezones"]) {
        let zones: Vec<String> = out
            .lines()
            .map(str::trim)
            .filter(|z| !z.is_empty())
            .map(str::to_string)
            .collect();
        if !zones.is_empty() {
            return zones;
        }
    }
    let mut zones = Vec::new();
    walk_zoneinfo(std::path::Path::new("/usr/share/zoneinfo"), "", &mut zones);
    if zones.is_empty() {
        zones.push("UTC".to_string());
    }
    zones
}

/// Read the effective state: system clock facts overlaid with the stored
/// 24-hour preference.
pub fn get(store: &SettingsStore) -> DateTimeState {
    DateTimeState {
        ntp: ntp_active(),
        timezone: current_timezone(),
        use_24h: store
            .get(DOMAIN, KEY_USE_24H)
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        timezones: timezone_list(),
    }
}

/// Apply a timezone after validating it against the real list, then
/// return the effective state.
pub fn set_timezone(
    store: &Arc<Mutex<SettingsStore>>,
    timezone: &str,
) -> Result<DateTimeState, String> {
    if timezone.is_empty() {
        return Err("datetime set failed: timezone must not be empty".to_string());
    }
    if !timezone_list().iter().any(|z| z == timezone) {
        return Err(format!("datetime set failed: unknown timezone {:?}", timezone));
    }
    networkkit::util::run("timedatectl", &["set-timezone", timezone])
        .map_err(|e| format!("datetime set failed: {}", e))?;
    let guard = store
        .lock()
        .map_err(|_| "datetime set failed: store is locked".to_string())?;
    Ok(get(&guard))
}

/// Persist the 24-hour display preference and return the effective state.
pub fn set_24h(store: &Arc<Mutex<SettingsStore>>, use_24h: bool) -> Result<DateTimeState, String> {
    let mut guard = store
        .lock()
        .map_err(|_| "datetime set failed: store is locked".to_string())?;
    guard.set(DOMAIN, KEY_USE_24H, serde_json::json!(use_24h));
    let effective = get(&guard);
    guard
        .save()
        .map_err(|e| format!("datetime set failed: store save failed: {}", e))?;
    Ok(effective)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn memory_store() -> Arc<Mutex<SettingsStore>> {
        Arc::new(Mutex::new(SettingsStore::new(PathBuf::from(
            "/tmp/tontoo-settings-datetime-test.json",
        ))))
    }

    #[test]
    fn defaults_when_store_empty() {
        let store = memory_store();
        let guard = store.lock().unwrap();
        let state = get(&guard);
        assert!(!state.use_24h);
        // Real, non-empty timezone list on any systemd or zoneinfo host.
        assert!(!state.timezones.is_empty());
        assert!(state.timezones.iter().all(|z| is_zone_file(z)));
    }

    #[test]
    fn current_timezone_is_plausible() {
        let zone = current_timezone();
        assert!(!zone.is_empty());
        assert!(!zone.contains(".."));
    }

    #[test]
    fn set_24h_roundtrip() {
        let store = memory_store();
        let state = set_24h(&store, true).unwrap();
        assert!(state.use_24h);
        let guard = store.lock().unwrap();
        assert!(get(&guard).use_24h);
        drop(guard);
        let state = set_24h(&store, false).unwrap();
        assert!(!state.use_24h);
    }

    #[test]
    fn rejects_empty_and_unknown_timezones() {
        let store = memory_store();
        assert!(set_timezone(&store, "").is_err());
        assert!(set_timezone(&store, "../etc/passwd").is_err());
        assert!(set_timezone(&store, "Mars/Olympus_Mons").is_err());
    }
    #[test]
    fn zone_file_filter() {
        assert!(is_zone_file("Europe/Berlin"));
        assert!(is_zone_file("UTC"));
        assert!(!is_zone_file("zone.tab"));
        assert!(!is_zone_file("posix/Europe/Berlin"));
        assert!(!is_zone_file("right/UTC"));
        assert!(!is_zone_file(""));
    }

    #[test]
    fn ensure_ntp_without_timedatectl_errors() {
        if !networkkit::util::tool_available("timedatectl") {
            assert!(ensure_ntp().is_err());
        }
    }
}
