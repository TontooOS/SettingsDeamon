# Store

`SettingsStore` is a two-level map of domain to key to JSON value with file
persistence. It mirrors the macOS `~/Library/Preferences/<domain>` idea with
one JSON file per daemon instead of one plist per domain for the basis.

## Data Format

On-disk layout is `object<string, object<string, json>>`:

```json
{
  "appearance": { "theme": "dark" },
  "locale": { "lang": "en_us" }
}
```

## API

### Constructors

```rust
impl SettingsStore {
  pub fn new(path: PathBuf) -> Self;
  pub fn path(&self) -> &Path;
}
```

Creates an empty store bound to `path`. Does not touch the filesystem.

### Persistence

```rust
impl SettingsStore {
  pub fn load(&mut self) -> io::Result<()>;
  pub fn save(&self) -> io::Result<()>;
}
```

- `load` clears and fills the store. Returns `Ok` on a missing file.
  Returns `Err` on unreadable files or invalid JSON and leaves content
  untouched.
- `save` creates parent directories and writes pretty JSON. Returns `Err`
  when the write fails.

### Values

```rust
impl SettingsStore {
  pub fn get(&self, domain: &str, key: &str) -> Option<&Value>;
  pub fn set(&mut self, domain: &str, key: &str, value: Value);
  pub fn remove(&mut self, domain: &str, key: &str) -> bool;
  pub fn domains(&self) -> Vec<String>;
}
```

- `get` returns `None` when the domain or key does not exist.
- `set` creates the domain map on demand.
- `remove` returns `true` when a value was removed and drops domains left
  empty.
- `domains` returns sorted domain names.

## Usage / Example

```rust
use settings_daemon::SettingsStore;
use std::path::PathBuf;

let mut store = SettingsStore::new(PathBuf::from("/tmp/settings.json"));
store.set("appearance", "theme", serde_json::json!("dark"));
store.save()?;
store.load()?;
assert_eq!(store.get("appearance", "theme"), Some(&serde_json::json!("dark")));
```

## Cross References

- [Daemon.md](Daemon.md) – daemon load/save wiring
- [Socket.md](Socket.md) – future get/set dispatch on top of the store
