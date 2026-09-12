# Datetime

The daemon owns date & time control. The `datetime` backend reads NTP
state and the timezone via `timedatectl`, applies timezones with
`timedatectl set-timezone`, and persists the 24-hour display preference
in the `datetime` store domain. `datetime_get` is a public read op;
`datetime_set_timezone` and `datetime_set_24h` are private write ops
reserved for the Settings app (`com.tontoo.systemsettings`).

## Public read ops

| Op | Result |
|---|---|
| `datetime_get` | `{"ntp": bool, "timezone": "...", "use_24h": bool, "timezones": [...]}` |

`timezones` is the real zone list from `timedatectl list-timezones`
with a `/usr/share/zoneinfo` walk as fallback (never empty, so clients
never show a load error). `timezone` falls back to the `/etc/localtime`
symlink target, then `UTC`. `ntp` is false when `timedatectl` is
unavailable.

## Private write ops

| Op | Params | Result |
|---|---|---|
| `datetime_set_timezone` | `timezone` | The effective `DateTimeState` |
| `datetime_set_24h` | `use_24h` | The effective `DateTimeState` |

Rules:

- `datetime_set_timezone` validates against the real list and rejects
  empty, unknown or path-like values before touching the system.
- The 24-hour preference is validated storage only (boolean); corrupt
  values fall back to `false`.
- Missing params return `ok: false`, never a partial result.
- `ensure_ntp` runs at daemon startup (best effort): automatic time
  sync stays on, the Settings app offers no way to turn it off.

## API

```rust
pub fn get(store: &SettingsStore) -> DateTimeState
pub fn set_timezone(store: &Arc<Mutex<SettingsStore>>, timezone: &str) -> Result<DateTimeState, String>
pub fn set_24h(store: &Arc<Mutex<SettingsStore>>, use_24h: bool) -> Result<DateTimeState, String>
pub fn ensure_ntp() -> Result<bool, String>
pub fn timezone_list() -> Vec<String>
pub const OP_DATETIME_GET: &str = "datetime_get";
pub const OP_DATETIME_SET_TIMEZONE: &str = "datetime_set_timezone";
pub const OP_DATETIME_SET_24H: &str = "datetime_set_24h";
```

- Every fallible function returns `Err(String)` on failure; the socket
  layer wraps it as `{"id": id, "ok": false, "error": "..."}`.

## Usage / Example

```bash
printf '{"id": 1, "op": "datetime_get"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 2, "op": "datetime_set_timezone", "params": {"timezone": "Europe/Berlin"}}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
```

```rust
use settings_daemon::datetime;

let state = datetime::set_24h(&store, true)?;
println!("{} (24h: {})", state.timezone, state.use_24h);
```

## Cross References

- [Socket.md](Socket.md) -- protocol frames carrying these ops
- [Daemon.md](Daemon.md) -- configuration and runtime model
- [Store.md](Store.md) -- `datetime` domain holding `use_24h`
