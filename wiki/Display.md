# Display

The display backend persists brightness (0-100), night light and
per-output modes in the `display` store domain; live output data
(modes, current mode) always comes from the compositor. Socket ops:
`display_get` is a public read op, `display_set` is a private write op
reserved for the Settings app (`com.tontoo.systemsettings`), like the
`wifi_*` write ops.

`display_set` forwards to the compositor first and persists after
(strict like `wallpaper_apply`): nothing is persisted when the
compositor is unreachable. The daemon pushes the stored settings to the
compositor on startup (best effort), restoring brightness, night light
and output modes after reboot.

## Data Format

Stored keys under the `display` domain:

```json
{
  "display": {
    "brightness": 80,
    "night_light": false,
    "HDMI-1": { "width": 1920, "height": 1080, "refresh": 120 }
  }
}
```

Missing brightness falls back to `100`, night light to off, unknown
output shapes to no stored mode.

## API

```rust
pub struct DisplayOutput {
  pub name: String,
  pub modes: Vec<DisplayMode>,
  pub current: Option<DisplayMode>,
}
```

```rust
pub struct DisplayState {
  pub outputs: Vec<DisplayOutput>,
  pub brightness: u32,
  pub night_light: bool,
}
```

```rust
pub fn get(store: &SettingsStore) -> Result<DisplayState, String>;
pub fn set(
  store: &Arc<Mutex<SettingsStore>>,
  output: Option<&str>,
  width: Option<i64>,
  height: Option<i64>,
  refresh: Option<u32>,
  brightness: Option<f64>,
  night_light: Option<bool>,
) -> Result<DisplayState, String>;
pub fn push_to_compositor(store: &SettingsStore) -> Result<(), String>;
```

- `get` requires the compositor (no modes without it) and overlays the
  stored brightness and night light.
- `set` validates ranges (brightness 0-100, refresh 1-1000 Hz, positive
  sizes) before touching anything, forwards live, then persists
  brightness/night light plus the effective mode of the target output.
- Op names live with the backend
  (`settings_daemon::display::OP_DISPLAY_*`).

## Usage / Example

```bash
printf '{"id": 1, "op": "display_get"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 2, "op": "display_set", "params": {"brightness": 80.0}}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 3, "op": "display_set", "params": {"refresh": 120}}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
```

## Cross References

- [Store.md](Store.md) – `display` domain backing the settings
- [Socket.md](Socket.md) – `display_get`/`display_set` dispatch
- [Daemon.md](Daemon.md) – startup push wiring
