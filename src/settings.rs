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
    /// Start the game server with `--debug-log` (script failure diagnostics).
    pub debug_log: bool,

    pub asset_mode: AssetMode,
    pub copy_loose_data: bool,
    pub copy_bgm_ai: bool,

    pub server_repo: String,
    pub client_repo: String,
    pub remote_client_repo: String,

    /// Copy the ROenglishRE base translation and the ROenglishPRE overlay over
    /// the client's loose data files.
    pub install_translation: bool,
    pub translation_re_repo: String,
    pub translation_pre_repo: String,
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
            debug_log: false,
            asset_mode: AssetMode::Copy,
            copy_loose_data: true,
            copy_bgm_ai: false,
            server_repo: "https://github.com/pts1667/rust-ro.git".into(),
            client_repo: "https://github.com/MrAntares/roBrowserLegacy.git".into(),
            remote_client_repo: "https://github.com/Flux159/roBrowserLegacy-RemoteClient-Rust.git"
                .into(),
            install_translation: system_language_is_english(),
            translation_re_repo: "https://github.com/zackdreaver/ROenglishRE.git".into(),
            translation_pre_repo: "https://github.com/zackdreaver/ROenglishPRE.git".into(),
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

    pub fn server_args(&self) -> Vec<String> {
        self.debug_log.then(|| "--debug-log".to_string()).into_iter().collect()
    }

    pub fn server_dir(&self) -> PathBuf {
        self.install_dir.join("rust-ro")
    }

    pub fn client_dir(&self) -> PathBuf {
        self.install_dir.join("roBrowserLegacy")
    }

    pub fn remote_client_dir(&self) -> PathBuf {
        self.install_dir.join("remote-client")
    }

    pub fn translation_re_dir(&self) -> PathBuf {
        self.install_dir.join("ROenglishRE")
    }

    pub fn translation_pre_dir(&self) -> PathBuf {
        self.install_dir.join("ROenglishPRE")
    }

    /// Directory that holds the ROenglishRE client files (`Ragnarok/data`, ...).
    pub fn translation_re_source(&self) -> PathBuf {
        self.translation_re_dir().join("Ragnarok")
    }

    /// Directory that holds the ROenglishPRE client files (`data`, `System`).
    pub fn translation_pre_source(&self) -> PathBuf {
        self.translation_pre_dir()
    }

    /// Marker written once a translation has been copied into the webroot.
    pub fn translation_re_marker(&self) -> PathBuf {
        self.install_dir.join("translation-re.json")
    }

    pub fn translation_pre_marker(&self) -> PathBuf {
        self.install_dir.join("translation-pre.json")
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

    pub fn translation_re_cloned(&self) -> bool {
        self.translation_re_source().join("data").is_dir()
    }

    pub fn translation_pre_cloned(&self) -> bool {
        self.translation_pre_dir().join("data").is_dir()
    }

    /// True once both translation layers were copied over the webroot data.
    pub fn translation_applied(&self) -> bool {
        self.translation_re_marker().is_file() && self.translation_pre_marker().is_file()
    }

    pub fn translation_re_applied(&self) -> bool {
        self.translation_re_marker().is_file()
    }

    pub fn translation_pre_applied(&self) -> bool {
        self.translation_pre_marker().is_file()
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

/// Whether the system's preferred UI language is English. Used to decide the
/// default for the optional data translation on brand new configurations.
pub fn system_language_is_english() -> bool {
    sys_locale::get_locale()
        .map(|locale| locale.to_ascii_lowercase().starts_with("en"))
        .unwrap_or(false)
}

#[cfg(test)]
mod debug_log_tests {
    use super::Settings;

    #[test]
    fn server_gets_the_debug_log_flag_only_when_enabled() {
        let mut settings = Settings::default();
        assert!(settings.server_args().is_empty());
        settings.debug_log = true;
        assert_eq!(settings.server_args(), ["--debug-log"]);
    }

    #[test]
    fn settings_files_without_the_option_keep_it_off() {
        let settings: Settings = serde_json::from_str(r#"{"game_port": 7000}"#).unwrap();
        assert!(!settings.debug_log);
    }
}
