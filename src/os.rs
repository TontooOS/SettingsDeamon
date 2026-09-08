use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Defaults (current release)
// ---------------------------------------------------------------------------

pub const DEFAULT_OS_NAME: &str = "TontooOS";
pub const DEFAULT_OS_DISPLAY_NAME: &str = "TontooOS Seal";
pub const DEFAULT_OS_CODENAME: &str = "Seal";
pub const DEFAULT_OS_VERSION: &str = "26.1.0";

/// Release file the daemon prefers over compiled defaults. BaseOS ships this
/// file, so new releases never need a daemon rebuild.
pub const RELEASE_FILE: &str = "/etc/tontoo-release";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// OS identity facts written to `os.fico`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsInfo {
  pub name: String,
  pub display_name: String,
  pub codename: String,
  pub version: String,
  /// True when the beta channel is activated on this installation.
  pub beta: bool,
}

impl Default for OsInfo {
  fn default() -> Self {
    Self {
      name: DEFAULT_OS_NAME.to_string(),
      display_name: DEFAULT_OS_DISPLAY_NAME.to_string(),
      codename: DEFAULT_OS_CODENAME.to_string(),
      version: DEFAULT_OS_VERSION.to_string(),
      beta: false,
    }
  }
}

// ---------------------------------------------------------------------------
// Collection entry points
// ---------------------------------------------------------------------------

/// Collect OS facts.
///
/// Override chain, first hit wins per field: `RELEASE_FILE`
/// (`/etc/tontoo-release`, `KEY=value` lines) overrides compiled defaults,
/// `TONTOO_OS_*` environment variables override both (intended for tests).
pub fn collect() -> OsInfo {
  let mut info = OsInfo::default();

  if let Ok(raw) = std::fs::read_to_string(RELEASE_FILE) {
    apply_release_file(&mut info, &raw);
  }
  apply_env(&mut info);
  info
}

/// Collect and write `os.fico` to `path`, creating parent directories.
/// Returns the snapshot that was written. Returns `Err` only on real I/O
/// errors.
pub fn write_os_fico(path: &Path) -> io::Result<OsInfo> {
  let info = collect();
  let doc = info.to_fico();
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent)?;
  }
  doc
    .write_to_file(path)
    .map_err(|e| io::Error::other(e.to_string()))?;
  Ok(info)
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// Parse `KEY=value` lines (`os-release` style, quotes optional).
/// Unknown keys are ignored.
fn apply_release_file(info: &mut OsInfo, raw: &str) {
  for line in raw.lines() {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
      continue;
    }
    let (key, value) = match line.split_once('=') {
      Some(pair) => (pair.0.trim(), unquote(pair.1.trim())),
      None => continue,
    };
    match key {
      "NAME" => info.name = value,
      "DISPLAY_NAME" => info.display_name = value,
      "CODENAME" => info.codename = value,
      "VERSION" => info.version = value,
      "BETA" => info.beta = parse_bool(&value),
      _ => {}
    }
  }
}

fn apply_env(info: &mut OsInfo) {
  if let Ok(v) = std::env::var("TONTOO_OS_NAME") {
    if !v.is_empty() {
      info.name = v;
    }
  }
  if let Ok(v) = std::env::var("TONTOO_OS_DISPLAY_NAME") {
    if !v.is_empty() {
      info.display_name = v;
    }
  }
  if let Ok(v) = std::env::var("TONTOO_OS_CODENAME") {
    if !v.is_empty() {
      info.codename = v;
    }
  }
  if let Ok(v) = std::env::var("TONTOO_OS_VERSION") {
    if !v.is_empty() {
      info.version = v;
    }
  }
  if let Ok(v) = std::env::var("TONTOO_OS_BETA") {
    info.beta = parse_bool(&v);
  }
}

fn unquote(raw: &str) -> String {
  if raw.len() >= 2
    && ((raw.starts_with('"') && raw.ends_with('"'))
      || (raw.starts_with('\'') && raw.ends_with('\'')))
  {
    raw[1..raw.len() - 1].to_string()
  } else {
    raw.to_string()
  }
}

fn parse_bool(raw: &str) -> bool {
  matches!(raw.trim().to_lowercase().as_str(), "1" | "true" | "yes")
}

// ---------------------------------------------------------------------------
// fico serialization
// ---------------------------------------------------------------------------

impl OsInfo {
  /// Serialize to a `FishDocument` with a single `os` section.
  pub fn to_fico(&self) -> sdk::FishFile::FishDocument {
    let mut doc = sdk::FishFile::FishDocument::new();
    doc.set("os.name", self.name.as_str());
    doc.set("os.display_name", self.display_name.as_str());
    doc.set("os.codename", self.codename.as_str());
    doc.set("os.version", self.version.as_str());
    doc.set("os.beta", self.beta);
    doc
  }
}

// ---------------------------------------------------------------------------
// Tests (fixture-based)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn defaults_are_seal_26_1() {
    let info = OsInfo::default();
    assert_eq!(info.display_name, "TontooOS Seal");
    assert_eq!(info.codename, "Seal");
    assert_eq!(info.version, "26.1.0");
    assert!(!info.beta);
  }

  #[test]
  fn release_file_overrides_defaults() {
    let mut info = OsInfo::default();
    apply_release_file(
      &mut info,
      "# TontooOS release\nNAME=TontooOS\nDISPLAY_NAME=\"TontooOS Seal\"\nCODENAME=Seal\nVERSION=26.2.0\nBETA=true\nUNKNOWN_KEY=ignored\n",
    );
    assert_eq!(info.version, "26.2.0");
    assert!(info.beta);
    assert_eq!(info.display_name, "TontooOS Seal");
  }

  #[test]
  fn release_file_beta_false_variants() {
    let mut info = OsInfo::default();
    info.beta = true;
    apply_release_file(&mut info, "BETA=0\n");
    assert!(!info.beta);
  }

  #[test]
  fn to_fico_roundtrip() {
    let info = OsInfo::default();
    let doc = info.to_fico();
    assert_eq!(
      doc.get("os.display_name").and_then(|v| v.as_str()),
      Some("TontooOS Seal")
    );
    assert_eq!(doc.get("os.codename").and_then(|v| v.as_str()), Some("Seal"));
    assert_eq!(doc.get("os.version").and_then(|v| v.as_str()), Some("26.1.0"));
    assert_eq!(doc.get("os.beta").and_then(|v| v.as_bool()), Some(false));
    let reparsed = sdk::FishFile::FishDocument::parse(&doc.to_string()).unwrap();
    assert_eq!(
      reparsed.get("os.version").and_then(|v| v.as_str()),
      Some("26.1.0")
    );
  }
}
