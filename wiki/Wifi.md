# WiFi

The daemon owns WiFi control. The `wifi` backend drives NetworkManager
through NetworkKit's direct reads and persists known networks (SSID,
security, timestamp, `auto_join`, encrypted password) as `KnownWifi`
entities in a system-wide CoreData store at
`/System/Preferences/com.tontoo.wifi/storage.fico`, shared by all users.
NetworkKit's read-only `Wifi` client queries this backend; write
operations have no public client library and are reserved for the
Settings app (`com.tontoo.systemsettings`). On startup the daemon
auto-joins the most recently used visible known network (`auto_join`).

## Public read ops

| Op | Result |
|---|---|
| `wifi_list` | `{"networks": [...]}` with `known: true` for stored SSIDs, `known: false` otherwise |
| `wifi_status` | `{"enabled": bool, "status": WifiStatus\|null, "available": bool}` |
| `wifi_known_list` | `{"networks": [...]}` with `ssid`, `security`, `last_connected`, `auto_join` (never passwords) |

`wifi_list` runs an active rescan on every call. `enabled` reflects
`nmcli radio wifi`; `status` is the active connection (same shape as
NetworkKit `WifiStatus`) or `null` when disconnected. `available` is
false when no wireless adapter exists; then `enabled` is false and
`status` is null instead of an error so clients can show the
no-hardware message.

## Private write ops

| Op | Params | Result |
|---|---|---|
| `wifi_connect` | `ssid`, optional `password`, optional `hidden` | The verified `WifiStatus` |
| `wifi_disconnect` | none | `{"disconnected": true}` |
| `wifi_enable` | none | `{"enabled": true}` |
| `wifi_disable` | none | `{"enabled": false}` |
| `wifi_forget` | `ssid` | `{"forgotten": bool}` (true when a profile or store entry was removed) |

Rules:

- `wifi_connect` verifies the association against the requested SSID and
  stores the network as known on success (SSID, security, timestamp,
  `auto_join: true`, encrypted password for secured networks so the
  daemon can auto-join at startup).
- `wifi_forget` deletes the NetworkManager profile (best effort) and the
  CoreData entry.
- `wifi_known_list` is the only way clients read known networks; it
  never exposes passwords.
- `auto_join` runs at daemon startup: when an adapter exists, the radio
  is on and nothing is connected, it joins the most recently used
  visible known network with `auto_join` set. Every degraded state
  (no adapter, radio off, nothing known) yields `Ok(None)`.
- Missing or empty `ssid` params return `ok: false`, never a partial result.

## API

```rust
pub fn list() -> Result<Vec<WifiNetwork>, String>
pub fn status() -> Result<serde_json::Value, String>
pub fn known_list() -> Result<Vec<KnownNetwork>, String>
pub fn connect(ssid: &str, password: Option<&str>, hidden: bool) -> Result<WifiStatus, String>
pub fn disconnect() -> Result<(), String>
pub fn set_enabled(enabled: bool) -> Result<(), String>
pub fn forget(ssid: &str) -> Result<bool, String>
pub fn auto_join() -> Result<Option<WifiStatus>, String>
pub fn known_ssids() -> Result<HashSet<String>, String>
pub fn known_networks() -> Result<Vec<KnownNetwork>, String>
pub fn mark_known(networks: Vec<WifiNetwork>, known: &HashSet<String>) -> Vec<WifiNetwork>
```

- All functions are stateless; the CoreData container opens per call.
- `mark_known` is a pure helper that flags scan results against stored SSIDs.
- Every function returns `Err(String)` on failure; the socket layer wraps
  it as `{"id": id, "ok": false, "error": "..."}`.

```rust
pub const DAEMON_BUNDLE_ID: &str = "com.tontoo.settingsdaemon";
pub const SYSTEM_WIFI_BUNDLE_ID: &str = "com.tontoo.wifi";
pub const KNOWN_WIFI_ENTITY: &str = "KnownWifi";
```

## Usage / Example

```bash
printf '{"id": 1, "op": "wifi_list"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 2, "op": "wifi_status"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
```

```rust
use settings_daemon::wifi;

let networks = wifi::list()?;
for net in &networks {
    println!("{} {}% {}", net.ssid, net.signal_pct, if net.known { "known" } else { "unknown" });
}
```

## Cross References

- [Socket.md](Socket.md) -- protocol frames carrying these ops
- [Daemon.md](Daemon.md) -- configuration and runtime model
- NetworkKit `Wifi.md` -- read-only client querying this backend
