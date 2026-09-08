use std::path::PathBuf;

/// Default unix socket path for the settings daemon.
/// Mirrors the FishPerms convention (`/run/<service>.sock`).
pub const DEFAULT_SOCKET_PATH: &str = "/run/tontoo-settings.sock";

/// Default system-wide settings store path (macOS-style Preferences location).
pub const DEFAULT_STORE_PATH: &str = "/Library/Preferences/TontooSettings/settings.json";

/// Default hardware snapshot path (Fish Config format, refreshed at startup).
pub const DEFAULT_SYS_FICO_PATH: &str = "/Library/Preferences/SystemConfiguration/sys.fico";

/// Default OS identity path (Fish Config format, refreshed at startup).
pub const DEFAULT_OS_FICO_PATH: &str = "/Library/Preferences/SystemConfiguration/os.fico";

/// Runtime configuration for the settings daemon.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
  pub socket_path: PathBuf,
  pub store_path: PathBuf,
  pub sys_fico_path: PathBuf,
  pub os_fico_path: PathBuf,
}

impl Default for DaemonConfig {
  fn default() -> Self {
    Self {
      socket_path: PathBuf::from(DEFAULT_SOCKET_PATH),
      store_path: PathBuf::from(DEFAULT_STORE_PATH),
      sys_fico_path: PathBuf::from(DEFAULT_SYS_FICO_PATH),
      os_fico_path: PathBuf::from(DEFAULT_OS_FICO_PATH),
    }
  }
}

impl DaemonConfig {
  /// Build configuration from environment with fallback to defaults.
  ///
  /// | Variable | Meaning |
  /// |---|---|---|
  /// | `SETTINGS_SOCKET` | Override for `socket_path` |
  /// | `SETTINGS_STORE` | Override for `store_path` |
  /// | `SETTINGS_SYS_FICO` | Override for `sys_fico_path` |
  /// | `SETTINGS_OS_FICO` | Override for `os_fico_path` |
  pub fn from_env() -> Self {
    let socket_path = std::env::var("SETTINGS_SOCKET")
      .ok()
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET_PATH));
    let store_path = std::env::var("SETTINGS_STORE")
      .ok()
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from(DEFAULT_STORE_PATH));
    let sys_fico_path = std::env::var("SETTINGS_SYS_FICO")
      .ok()
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from(DEFAULT_SYS_FICO_PATH));
    let os_fico_path = std::env::var("SETTINGS_OS_FICO")
      .ok()
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from(DEFAULT_OS_FICO_PATH));
    Self {
      socket_path,
      store_path,
      sys_fico_path,
      os_fico_path,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_paths_are_macos_style() {
    let config = DaemonConfig::default();
    assert_eq!(config.socket_path, PathBuf::from(DEFAULT_SOCKET_PATH));
    assert_eq!(config.store_path, PathBuf::from(DEFAULT_STORE_PATH));
    assert_eq!(config.sys_fico_path, PathBuf::from(DEFAULT_SYS_FICO_PATH));
    assert_eq!(config.os_fico_path, PathBuf::from(DEFAULT_OS_FICO_PATH));
  }
}
