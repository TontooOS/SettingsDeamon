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
  "customize": { "wallpaper": "THAOELAKE", "accent": "multicolor", "theme": "dark", "glass": "glass", "revision": 3 }
}
```

- `wallpaper` is a wallpaper pack id (e.g. `"THAOELAKE"`, see the
  `BaseOS/wallpapers` packs staged at `/System/User/Wallpapers`).
- `accent` is one of `multicolor` (default element, renders blue), `blue`,
  `red`, `orange`, `yellow`, `green`, `teal`, `cyan`, `indigo`, `purple`,
  `purple2`, `pink`, `gray` (Settings app palette).
- `theme` is `dark` or `light`.
- `glass` is the LiquidGlass slider: `much`, `glass` (default) or `less`.
- `revision` bumps on every successful `customize_set` so clients can poll
  for changes.

Missing keys fall back to `THAOELAKE` / `multicolor` / `dark` / `glass` /
`0`. Invalid stored values (unknown accent/theme/glass, empty wallpaper)
fall back to the defaults on read and are rejected with an error on write,
so a corrupt store file can never produce an invalid reply.

## API

```rust
pub struct CustomizeSettings {
  pub wallpaper: String,
  pub accent: String,
  pub theme: String,
  pub glass: String,
  pub revision: u64,
}
```

```rust
pub enum GlassAmount {
  Much,
  Glass,
  Less,
}
```

`GlassAmount::from_str` returns `None` for unknown input;
`CustomizeSettings::glass_amount` falls back to `Glass`. Exposes `as_str`
round-tripping the store spelling.

```rust
pub enum ThemeMode {
  Dark,
  Light,
}
```

```rust
pub enum AccentColor {
  Multicolor,
  Blue,
  Red,
  Orange,
  Yellow,
  Green,
  Teal,
  Cyan,
  Indigo,
  Purple,
  Purple2,
  Pink,
  Gray,
}
```

`ThemeMode::from_str` returns `None` for unknown input;
`CustomizeSettings::theme_mode` falls back to `Dark`.
`AccentColor::from_str` returns `None` for unknown input;
`CustomizeSettings::accent_color` falls back to `Multicolor`.
Both expose `as_str` round-tripping the store spelling.

```rust
pub fn get(store: &SettingsStore) -> CustomizeSettings;
pub fn set(
  store: &Arc<Mutex<SettingsStore>>,
  wallpaper: Option<&str>,
  accent: Option<&str>,
  theme: Option<&str>,
  glass: Option<&str>,
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

`set_customize.py` (repo root) wraps the same frames like the Settings
app would, with readable output:

```bash
python3 set_customize.py --get
python3 set_customize.py --theme light --accent blue
python3 set_customize.py --glass much
```

## Cross References

- [Store.md](Store.md) – backing `customize` domain and persistence
- [Socket.md](Socket.md) – `customize_get`/`customize_set` dispatch
- [Daemon.md](Daemon.md) – configuration and runtime model
