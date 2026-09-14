use std::path::PathBuf;
use std::sync::OnceLock;

/// Returns the path to the user's home directory.
pub fn home_dir() -> &'static PathBuf {
    static HOME_DIR: OnceLock<PathBuf> = OnceLock::new();
    HOME_DIR.get_or_init(|| dirs::home_dir().expect("failed to determine home directory"))
}

/// Returns the path to the user's download directory.
pub fn download_dir() -> &'static PathBuf {
    static DOWNLOAD_DIR: OnceLock<PathBuf> = OnceLock::new();
    DOWNLOAD_DIR
        .get_or_init(|| dirs::download_dir().expect("failed to determine download directory"))
}

/// Returns the path to the configuration directory used by Goop.
pub fn config_dir() -> &'static PathBuf {
    static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();
    CONFIG_DIR.get_or_init(|| {
        if cfg!(target_os = "windows") {
            return dirs::config_dir()
                .expect("failed to determine RoamingAppData directory")
                .join("Goop");
        }

        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            return if let Ok(flatpak_xdg_config) = std::env::var("FLATPAK_XDG_CONFIG_HOME") {
                flatpak_xdg_config.into()
            } else {
                dirs::config_dir().expect("failed to determine XDG_CONFIG_HOME directory")
            }
            .join("goop");
        }

        home_dir().join(".config").join("goop")
    })
}

/// Returns the path to the support directory used by Goop.
pub fn support_dir() -> &'static PathBuf {
    static SUPPORT_DIR: OnceLock<PathBuf> = OnceLock::new();
    SUPPORT_DIR.get_or_init(|| {
        if cfg!(target_os = "macos") {
            return home_dir().join("Library/Application Support/Goop");
        }

        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            return if let Ok(flatpak_xdg_data) = std::env::var("FLATPAK_XDG_DATA_HOME") {
                flatpak_xdg_data.into()
            } else {
                dirs::data_local_dir().expect("failed to determine XDG_DATA_HOME directory")
            }
            .join("goop");
        }

        if cfg!(target_os = "windows") {
            return dirs::data_local_dir()
                .expect("failed to determine LocalAppData directory")
                .join("goop");
        }

        config_dir().clone()
    })
}
