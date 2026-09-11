# Wallpaper

The wallpaper backend scans premade packs and user custom wallpapers and
persists the selection. `wallpaper_get` is a public read op;
`wallpaper_set_current`, `wallpaper_set_fill`, `wallpaper_add` and
`wallpaper_apply` are private write ops reserved for the Settings app
(`com.tontoo.systemsettings`), like the `wifi_*` write ops.
`wallpaper_apply` forwards the resolved file to the compositor for the
desktop crossfade; the other writes persist the selection only.

## Sources

Premade packs come from the system wallpapers directory
(`/System/User/Wallpapers`, `TONTOO_WALLPAPERS_DIR` override): one
subdirectory per pack with a `wallpaper.fish` manifest. The display name
is the manifest `name:` line (directory name fallback), the preview image
is the manifest `images: light:` file (first image file fallback). Packs
without any image are listed with an empty path.

User customs live in `~/Library/Preferences/com.tontoo.wallpaper/`
(`SETTINGS_WALLPAPER_DIR` override). Each upload is decoded (png, jpeg,
webp and every format the `image` crate reads) and stored as PNG with a
sanitized unique stem (`my-photo.png`, `my-photo-2.png`, ...). Customs are
listed from the `*.png` files on disk with display names mirrored in
`storage.fico` next to them (`customN: id/name/filename`), falling back
to the file stem. Premade packs sort in macOS release order (newest
first: Golden Gate, Tahoe, Tahoe Lake, Sequoia, ...; unknown ids last).

## Registry

Every added custom is registered in CoreData (`CustomWallpaper` entity
with `id`, `name`, `filename` under the daemon bundle id), mirroring the
known-network pattern in [Wifi.md](Wifi.md). The listing reads the files
plus `storage.fico`, so orphan files still show up.

The current wallpaper (`current_kind`/`current_id`) and the fill mode
(`fill`) persist in the `wallpaper` store domain. Unknown ids resolve to
no current wallpaper ("No wallpaper set"); unknown fill modes fall back
to `fill`. Fill modes are `fill`, `fit`, `stretch`, `center`, `tile`.

## API

```rust
pub struct WallpaperEntry {
  pub kind: String,
  pub id: String,
  pub name: String,
  pub path: String,
  pub path_dark: String,
}
```

`path` is the light (default) image, `path_dark` the dark variant (same
file when the pack ships only one image; customs always mirror `path`).

```rust
pub struct WallpaperState {
  pub current: Option<WallpaperEntry>,
  pub fill: String,
  pub customs: Vec<WallpaperEntry>,
  pub premade: Vec<WallpaperEntry>,
}
```

```rust
pub fn scan_premade() -> Vec<WallpaperEntry>;
pub fn scan_custom() -> Vec<WallpaperEntry>;
pub fn add(source: &Path, name: Option<&str>) -> Result<WallpaperEntry, String>;
pub fn state(store: &SettingsStore) -> WallpaperState;
pub fn set_current(store: &Arc<Mutex<SettingsStore>>, kind: &str, id: &str)
  -> Result<Option<WallpaperEntry>, String>;
pub fn set_fill(store: &Arc<Mutex<SettingsStore>>, fill: &str) -> Result<String, String>;
```

- `add` creates the customs directory on demand, converts to PNG,
  registers in CoreData and refreshes `storage.fico`. Returns `Err`
  without side effects (a half-written file is removed) when the source
  is missing, undecodable, or the registry fails.
- `set_current` resolves the kind/id against fresh scans and returns
  `Err` for unknown kinds or ids. `set_fill` rejects unknown modes.
- `apply` takes `light`, `dark` or `auto` (`auto` follows the
  `customize` theme), resolves the variant file, forwards it to the
  compositor socket (`COMPOSITOR_SOCKET` or
  `/run/tontoo-compositor.sock`, `set_wallpaper` op), then persists the
  selection. Nothing is persisted when the compositor is unreachable.
- Op names live with the backend
  (`settings_daemon::wallpaper::OP_WALLPAPER_*`).

## Usage / Example

```bash
printf '{"id": 1, "op": "wallpaper_get"}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 2, "op": "wallpaper_add", "params": {"path": "/tmp/photo.jpg"}}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 3, "op": "wallpaper_set_fill", "params": {"fill": "tile"}}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
printf '{"id": 4, "op": "wallpaper_apply", "params": {"kind": "premade", "id": "SONOMA", "variant": "auto"}}\n' | socat - UNIX-CONNECT:/run/tontoo-settings.sock
```

## Cross References

- [Store.md](Store.md) – `wallpaper` domain backing current and fill
- [Socket.md](Socket.md) – wallpaper op dispatch
- [Customize.md](Customize.md) – accent/theme customization backend
- [Wifi.md](Wifi.md) – CoreData registry pattern
