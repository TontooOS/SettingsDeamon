# Customize

The customize backend persists the desktop customization (wallpaper pack,
accent color, theme) in the `customize` store domain and serves it over the
socket. `customize_get` is a public read op, `customize_set` is a private
write op reserved for the Settings app (`com.tontoo.systemsettings`), like
the `wifi_*` write ops.

## Data Format

Stored keys under the `customize` domain:

```json
{
  "customize": { "wallpaper": "THAOELAKE", "accent": "orange", "theme": "dark" }
}
```

- `wallpaper` is a wallpaper pack id (e.g. `"THAOELAKE"`, see the
  `BaseOS/wallpapers` packs staged at `/System/User/Wallpapers`).
- `accent` is one of `orange`, `blue`, `green`, `purple`.
- `theme` is `dark` or `light`.

Missing keys fall back to `THAOELAKE` / `orange` / `dark`. Invalid stored
values (unknown accent/theme, empty wallpaper) fall back to the defaults
on read and are rejected with an error on write, so a corrupt store file
can never produce an invalid reply.

## API

```rust
pub struct CustomizeSettings {
  pub wallpaper: String,
  pub accent: String,
  pub theme: String,
}
```

```rust
pub fn get(store: &SettingsStore) -> CustomizeSettings;
pub fn set(
  store: &Arc<Mutex<SettingsStore>>,
  wallpaper: Option<&str>,
  accent: Option<&str>,
  theme: Option<&str>,
) -> Result<CustomizeSettings, String>;
```

- `get` overlays stored values on the defaults. Returns owned data, never
  borrows the store.
- `set` applies a partial update (`None` leaves the key untouched),
  validates every provided value before touching the store, persists with
  `SettingsStore::save` and returns the effective settings.
- `set` returns `Err` without touching the store when any provided value
  is invalid, when the mutex is poisoned, or when saving fails.

Op names live with the backend
(`settings_daemon::customize::OP_CUSTOMIZE_GET`,
`settings_daemon::customize::OP_CUSTOMIZE_SET`).

## Usage / Example

```rust
use settings_daemon::{CustomizeSettings, SettingsStore};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

let store = Arc::new(Mutex::new(SettingsStore::new(
  PathBuf::from("/tmp/settings.json"),
)));
let applied = settings_daemon::customize::set(
  &store,
  Some("SONOMA"),
  Some("blue"),
  None,
)?;
assert_eq!(applied.wallpaper, "SONOMA");
assert_eq!(applied.theme, "dark");
```

```bash
printf '{"id": 1, "op": "customize_get"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 2, "op": "customize_set", "params": {"theme": "light"}}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
```

## Cross References

- [Store.md](Store.md) – backing `customize` domain and persistence
- [Socket.md](Socket.md) – `customize_get`/`customize_set` dispatch
- [Daemon.md](Daemon.md) – configuration and runtime model
