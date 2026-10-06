//! Persisted launcher settings and the paths derived from them.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where game archives come from when the package is prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AssetMode {
    /// Copy the GRF archives into `<install>/webroot/resources/`.
    #[default]
    Copy,
    /// Leave the archives where they are and write a DATA.INI with absolute paths.
    Reference,
}

/// Settings are stored in `ro-sppack.json` in the current working directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub install_dir: PathBuf,
    pub assets_dir: PathBuf,

    pub server_host: String,
    /// Host/IP written into client configuration. Empty means "derive from bind host".
    pub public_host: String,
    pub game_port: u16,
    pub web_port: u16,
    pub packetver: u32,

    pub base_exp_rate: f32,
    pub job_exp_rate: f32,
    pub drop_rate: f32,
    pub drop_rate_card: f32,
    pub mob_density: f32,

    pub skip_intro: bool,
    pub auto_open_browser: bool,
    pub build_release: bool,

    pub asset_mode: AssetMode,
    pub copy_loose_data: bool,
    pub copy_bgm_ai: bool,

    pub server_repo: String,
    pub client_repo: String,
    pub remote_client_repo: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            install_dir: exe_dir().join("ro-sppack"),
            assets_dir: PathBuf::new(),
            server_host: "127.0.0.1".into(),
            public_host: String::new(),
            game_port: 6901,
            web_port: 3338,
            packetver: 20120307,
            base_exp_rate: 1.0,
            job_exp_rate: 1.0,
            drop_rate: 1.0,
            drop_rate_card: 1.0,
            mob_density: 1.0,
            skip_intro: true,
            auto_open_browser: true,
            build_release: true,
            asset_mode: AssetMode::Copy,
            copy_loose_data: true,
            copy_bgm_ai: false,
            server_repo: "https://github.com/pts1667/rust-ro.git".into(),
            client_repo: "https://github.com/MrAntares/roBrowserLegacy.git".into(),
            remote_client_repo: "https://github.com/Flux159/roBrowserLegacy-RemoteClient-Rust.git"
                .into(),
        }
    }
}

impl Settings {
    /// Path of the settings file: `ro-sppack.json` in the working directory.
    pub fn config_file() -> PathBuf {
        std::env::current_dir()
            .map(|dir| dir.join("ro-sppack.json"))
            .unwrap_or_else(|_| exe_dir().join("ro-sppack.json"))
    }

    pub fn load() -> Self {
        let path = Self::config_file();
        if let Ok(text) = std::fs::read_to_string(&path) {
            return serde_json::from_str(&text).unwrap_or_default();
        }
        // Migrate a settings file written next to the executable by older builds.
        let legacy = exe_dir().join("ro-sppack.json");
        if legacy != path
            && let Ok(text) = std::fs::read_to_string(&legacy)
        {
            let settings: Self = serde_json::from_str(&text).unwrap_or_default();
            let _ = settings.save();
            return settings;
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_file();
        let text =
            serde_json::to_string_pretty(self).map_err(|e| format!("serialize settings: {e}"))?;
        std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
    }

    // -- derived component paths -------------------------------------------------

    pub fn server_dir(&self) -> PathBuf {
        self.install_dir.join("rust-ro")
    }

    pub fn client_dir(&self) -> PathBuf {
        self.install_dir.join("roBrowserLegacy")
    }

    pub fn remote_client_dir(&self) -> PathBuf {
        self.install_dir.join("remote-client")
    }

    pub fn webroot(&self) -> PathBuf {
        self.install_dir.join("webroot")
    }

    pub fn resources_dir(&self) -> PathBuf {
        self.webroot().join("resources")
    }

    pub fn client_dist(&self) -> PathBuf {
        self.client_dir().join("dist").join("Web")
    }

    pub fn config_local_js(&self) -> PathBuf {
        self.webroot().join("Config.local.js")
    }

    pub fn server_config_json(&self) -> PathBuf {
        self.server_dir().join("config.json")
    }

    fn profile(&self) -> &'static str {
        if self.build_release {
            "release"
        } else {
            "debug"
        }
    }

    pub fn server_bin(&self) -> PathBuf {
        self.server_dir()
            .join("target")
            .join(self.profile())
            .join(format!("server{}", std::env::consts::EXE_SUFFIX))
    }

    pub fn remote_client_bin(&self) -> PathBuf {
        self.remote_client_dir()
            .join("target")
            .join(self.profile())
            .join(format!(
                "robrowser-remoteclient{}",
                std::env::consts::EXE_SUFFIX
            ))
    }

    /// Address the browser should use to reach the game server.
    pub fn connect_host(&self) -> String {
        let public = self.public_host.trim();
        if !public.is_empty() {
            return public.to_string();
        }
        match self.server_host.as_str() {
            "0.0.0.0" => "127.0.0.1".into(),
            "::" | "[::]" => "::1".into(),
            other => other.to_string(),
        }
    }

    /// Interface the asset/static server binds to.
    pub fn web_bind_host(&self) -> String {
        let host = self.server_host.trim();
        if host.is_empty() {
            "127.0.0.1".into()
        } else {
            host.to_string()
        }
    }

    // -- component detection -----------------------------------------------------

    pub fn server_cloned(&self) -> bool {
        self.server_dir().join("Cargo.toml").is_file()
    }

    pub fn client_cloned(&self) -> bool {
        self.client_dir().join("package.json").is_file()
    }

    pub fn remote_client_cloned(&self) -> bool {
        self.remote_client_dir().join("Cargo.toml").is_file()
    }

    pub fn server_built(&self) -> bool {
        self.server_bin().is_file()
    }

    pub fn client_deps_installed(&self) -> bool {
        self.client_dir().join("node_modules").is_dir()
    }

    pub fn client_built(&self) -> bool {
        self.client_dist().join("Online.js").is_file()
    }

    pub fn remote_client_built(&self) -> bool {
        self.remote_client_bin().is_file()
    }
}

/// Folder the launcher executable lives in.
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}
