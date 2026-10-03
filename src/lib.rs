pub mod config;
pub mod customize;
pub mod datetime;
pub mod display;
pub mod dns;
pub mod hardware;
pub mod json;
pub mod library;
pub mod locale;
pub mod os;
pub mod socket;
pub mod store;
pub mod wallpaper;
pub mod widgets;
pub mod wifi;
pub mod wired;

pub use config::{
  DaemonConfig, DEFAULT_OS_FICO_PATH, DEFAULT_SOCKET_PATH, DEFAULT_STORE_PATH, DEFAULT_SYS_FICO_PATH,
};
pub use customize::{
  AccentColor, CustomizeSettings, GlassAmount, ThemeMode, ACCENTS, GLASS_AMOUNTS, THEMES,
};
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
pub use widgets::{
  DOMAIN as WIDGETS_DOMAIN, EVENT_CONTENT_CHANGED, EVENT_WIDGETS_CHANGED, OP_CONTENT,
  OP_CONTENT_LIST, OP_LIST as OP_WIDGET_LIST, OP_REGISTER as OP_WIDGET_REGISTER,
  OP_UNREGISTER as OP_WIDGET_UNREGISTER,
};
pub use wifi::DAEMON_BUNDLE_ID;
