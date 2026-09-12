pub mod config;
pub mod customize;
pub mod display;
pub mod dns;
pub mod hardware;
pub mod library;
pub mod os;
pub mod socket;
pub mod store;
pub mod wallpaper;
pub mod wifi;
pub mod wired;

pub use config::{
  DaemonConfig, DEFAULT_OS_FICO_PATH, DEFAULT_SOCKET_PATH, DEFAULT_STORE_PATH, DEFAULT_SYS_FICO_PATH,
};
pub use customize::{CustomizeSettings, ACCENTS, THEMES};
pub use display::{DisplayMode, DisplayOutput, DisplayState};
pub use wallpaper::{WallpaperEntry, WallpaperState, FILL_MODES};

/// Process-wide lock serializing tests that mutate process environment
/// (`COMPOSITOR_SOCKET`, `TONTOO_WALLPAPERS_DIR`, ...). Every env-touching
/// test in every module must hold it, otherwise parallel tests flake.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub use hardware::{CpuInfo, GpuInfo, HardwareInfo, RamInfo, RamModule, RamType};
pub use library::{LibraryEntry, LibraryManager};
pub use os::OsInfo;
pub use store::SettingsStore;
pub use wifi::DAEMON_BUNDLE_ID;
