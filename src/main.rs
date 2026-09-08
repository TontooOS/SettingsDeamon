use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use settings_daemon::{hardware, os, DaemonConfig, LibraryManager, SettingsStore, socket};

fn main() {
  env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

  let config = DaemonConfig::from_env();
  log::info!("settings-daemon starting");
  log::info!("store at {:?}", config.store_path);
  log::info!("sys.fico at {:?}", config.sys_fico_path);
  log::info!("os.fico at {:?}", config.os_fico_path);
  log::info!("socket at {:?}", config.socket_path);

  let mut store = SettingsStore::new(config.store_path.clone());
  match store.load() {
    Ok(()) => log::info!("store loaded from {:?}", config.store_path),
    Err(e) => log::warn!("failed to load store from {:?} ({}), starting empty", config.store_path, e),
  }

  // Refresh the hardware snapshot on every start. Missing sources degrade
  // to Unknown, only real I/O errors are logged.
  match hardware::write_sys_fico(&config.sys_fico_path, true) {
    Ok(info) => log::info!(
      "sys.fico refreshed: cpu={:?} gpus={} ram_gb={} ram_type={}",
      info.cpu.name,
      info.gpus.len(),
      info.ram.total_gb,
      info.ram.ram_type.as_str()
    ),
    Err(e) => log::warn!("failed to refresh {:?} ({})", config.sys_fico_path, e),
  }

  // Refresh the OS identity file on every start.
  match os::write_os_fico(&config.os_fico_path) {
    Ok(info) => log::info!(
      "os.fico refreshed: {} {} beta={}",
      info.display_name,
      info.version,
      info.beta
    ),
    Err(e) => log::warn!("failed to refresh {:?} ({})", config.os_fico_path, e),
  }

  let store = Arc::new(Mutex::new(store));
  let libs = Arc::new(Mutex::new(LibraryManager::new()));

  if let Err(e) = socket::run_server(&config, store, libs) {
    log::error!("socket server failed: {}", e);
    std::process::exit(1);
  }
}

/// Resolve a user-level store path for future per-user overlay.
/// Basis helper, not wired into the daemon yet.
#[allow(dead_code)]
fn user_store_path() -> PathBuf {
  let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
  PathBuf::from(home).join(".config/tontoo/settings.json")
}
