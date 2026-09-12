# Locale

The daemon owns language & region control. The `locale` backend reads
and writes the system locale through `localectl`: system language
(English/German), region (date/number/currency/measurement formats) and
keyboard layout (X11 layout plus optional variant, with region-based
auto-detect). `locale_get` and `locale_keymap_variants` are public read
ops; the `locale_set_*` ops are private write ops reserved for the
Settings app (`com.tontoo.systemsettings`).

## Public read ops

| Op | Result |
|---|---|
| `locale_get` | `{"language", "region", "keymap", "keymap_variant", "auto_keymap", "languages", "regions", "keymaps"}` |
| `locale_keymap_variants` | `{"variants": [...]}` for `layout` param |

`languages` holds English and German only. `regions` is the full ISO
3166-1 territory list with English country names. `keymaps` holds X11
layouts (compact fallback when `localectl` is unavailable). Language
and region fall back to English/`US` when the system reports anything
else.

## Private write ops

| Op | Params | Result |
|---|---|---|
| `locale_set_language` | `language` (`en`/`de`) | The effective `LocaleState` |
| `locale_set_region` | `region` (territory, e.g. `DE`) | The effective `LocaleState` |
| `locale_set_keymap` | `layout`, optional `variant` | The effective `LocaleState` |
| `locale_set_auto_keymap` | `auto` | The effective `LocaleState` |

Rules:

- `locale_set_language` keeps the current region formats and writes
  `LANG` plus `LC_TIME`/`LC_NUMERIC`/`LC_MONETARY`/`LC_MEASUREMENT`.
- `locale_set_region` writes the `LC_*` format vars for the current
  language; with auto-detect on the keyboard follows the region mapping.
- `locale_set_keymap` validates layout and variant against the live
  X11 lists, applies via `set-x11-keymap` (console keymap best effort)
  and switches auto-detect off.
- Enabling auto-detect applies the mapped layout for the current region
  at once. Unknown layouts fall back to `us`.
- Missing or invalid params return `ok: false`, never a partial result.

## API

```rust
pub fn get(store: &SettingsStore) -> LocaleState
pub fn set_language(store: &Arc<Mutex<SettingsStore>>, language: &str) -> Result<LocaleState, String>
pub fn set_region(store: &Arc<Mutex<SettingsStore>>, region: &str) -> Result<LocaleState, String>
pub fn set_keymap(store: &Arc<Mutex<SettingsStore>>, layout: &str, variant: Option<&str>) -> Result<LocaleState, String>
pub fn set_auto_keymap(store: &Arc<Mutex<SettingsStore>>, auto: bool) -> Result<LocaleState, String>
pub fn keymap_variants(layout: &str) -> Vec<String>
pub const OP_LOCALE_GET: &str = "locale_get";
pub const OP_LOCALE_SET_LANGUAGE: &str = "locale_set_language";
pub const OP_LOCALE_SET_REGION: &str = "locale_set_region";
pub const OP_LOCALE_SET_KEYMAP: &str = "locale_set_keymap";
pub const OP_LOCALE_SET_AUTO_KEYMAP: &str = "locale_set_auto_keymap";
pub const OP_LOCALE_KEYMAP_VARIANTS: &str = "locale_keymap_variants";
```

- Every fallible function returns `Err(String)` on failure; the socket
  layer wraps it as `{"id": id, "ok": false, "error": "..."}`.

## Usage / Example

```bash
printf '{"id": 1, "op": "locale_get"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 2, "op": "locale_set_region", "params": {"region": "DE"}}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
```

```rust
use settings_daemon::locale;

let state = locale::set_language(&store, "de")?;
println!("{} in {}", state.language, state.region);
```

## Cross References

- [Socket.md](Socket.md) -- protocol frames carrying these ops
- [Daemon.md](Daemon.md) -- configuration and runtime model
- [Store.md](Store.md) -- `locale` domain holding `auto_keymap`
