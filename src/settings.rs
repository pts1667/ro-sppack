//! Persisted launcher settings and the paths derived from them.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where game archives come from when the package is prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AssetMode {
    /// Copy the GRF archives into `<install>/nostalro-client/data/`.
    #[default]
    Copy,
    /// Leave the archives where they are and reference them by absolute path.
    Reference,
}

/// Settings are stored in `ro-sppack.json` in the current working directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub install_dir: PathBuf,
    pub assets_dir: PathBuf,

    pub server_host: String,
    /// Host/IP written into the client configuration. Empty means "derive from bind host".
    pub public_host: String,
    pub game_port: u16,
    pub packetver: u32,

    pub base_exp_rate: f32,
    pub job_exp_rate: f32,
    pub drop_rate: f32,
    pub drop_rate_card: f32,
    pub mob_density: f32,

    pub build_release: bool,
    /// Start the game server with `--debug-log` (script failure diagnostics).
    pub debug_log: bool,

    pub asset_mode: AssetMode,
    pub copy_loose_data: bool,
    pub copy_bgm_ai: bool,

    pub server_repo: String,
    pub client_repo: String,

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
            packetver: 20120307,
            base_exp_rate: 1.0,
            job_exp_rate: 1.0,
            drop_rate: 1.0,
            drop_rate_card: 1.0,
            mob_density: 1.0,
            build_release: true,
            debug_log: false,
            asset_mode: AssetMode::Copy,
            copy_loose_data: true,
            copy_bgm_ai: false,
            server_repo: "https://github.com/pts1667/rust-ro.git".into(),
            client_repo: default_client_repo(),
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
            let mut settings: Self = serde_json::from_str(&text).unwrap_or_default();
            settings.migrate();
            return settings;
        }
        // Migrate a settings file written next to the executable by older builds.
        let legacy = exe_dir().join("ro-sppack.json");
        if legacy != path
            && let Ok(text) = std::fs::read_to_string(&legacy)
        {
            let mut settings: Self = serde_json::from_str(&text).unwrap_or_default();
            settings.migrate();
            let _ = settings.save();
            return settings;
        }
        Self::default()
    }

    /// Upgrade settings written by earlier launcher versions.
    fn migrate(&mut self) {
        // The roBrowserLegacy web client and the remote asset server were
        // replaced by the native nostalro client.
        if self.client_repo.trim().is_empty() || self.client_repo.contains("roBrowserLegacy") {
            self.client_repo = default_client_repo();
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_file();
        let text =
            serde_json::to_string_pretty(self).map_err(|e| format!("serialize settings: {e}"))?;
        std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
    }

    // -- derived component paths -------------------------------------------------

    pub fn server_args(&self) -> Vec<String> {
        self.debug_log
            .then(|| "--debug-log".to_string())
            .into_iter()
            .collect()
    }

    pub fn server_dir(&self) -> PathBuf {
        self.install_dir.join("rust-ro")
    }

    pub fn client_dir(&self) -> PathBuf {
        self.install_dir.join("nostalro-client")
    }

    /// GRF archives and loose client data used by the native client.
    pub fn client_data_dir(&self) -> PathBuf {
        self.client_dir().join("data")
    }

    /// Loose files that override archive entries (`data_dir` in config.json).
    pub fn client_extracted_dir(&self) -> PathBuf {
        self.client_data_dir().join("extracted")
    }

    /// BGM copied into the package when "copy BGM" is enabled.
    pub fn client_bgm_dir(&self) -> PathBuf {
        self.client_data_dir().join("BGM")
    }

    /// Manifest of the archive paths written into the client configuration.
    pub fn client_assets_manifest(&self) -> PathBuf {
        self.client_data_dir().join("ro-sppack-assets.json")
    }

    /// `config.json` of the native client (it also writes its settings there).
    pub fn client_config_json(&self) -> PathBuf {
        self.client_dir().join("config.json")
    }

    pub fn client_bin(&self) -> PathBuf {
        self.client_dir()
            .join("target")
            .join(self.profile())
            .join(format!("ragnarok-client{}", std::env::consts::EXE_SUFFIX))
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

    /// Marker written once a translation has been copied into the client data.
    pub fn translation_re_marker(&self) -> PathBuf {
        self.install_dir.join("translation-re.json")
    }

    pub fn translation_pre_marker(&self) -> PathBuf {
        self.install_dir.join("translation-pre.json")
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

    /// Address the client should use to reach the game server.
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

    // -- component detection -----------------------------------------------------

    pub fn server_cloned(&self) -> bool {
        self.server_dir().join("Cargo.toml").is_file()
    }

    pub fn client_cloned(&self) -> bool {
        self.client_dir().join("Cargo.toml").is_file()
    }

    pub fn translation_re_cloned(&self) -> bool {
        self.translation_re_source().join("data").is_dir()
    }

    pub fn translation_pre_cloned(&self) -> bool {
        self.translation_pre_dir().join("data").is_dir()
    }

    /// True once both translation layers were copied over the client data.
    pub fn translation_applied(&self) -> bool {
        self.translation_re_applied() && self.translation_pre_applied()
    }

    pub fn translation_re_applied(&self) -> bool {
        self.translation_marker_matches(&self.translation_re_marker())
    }

    pub fn translation_pre_applied(&self) -> bool {
        self.translation_marker_matches(&self.translation_pre_marker())
    }

    /// A marker only counts when it was written for the current destination.
    /// Markers from the old webroot layout (or a moved install) are ignored, so
    /// the translation is re-applied to the native client's data folder.
    fn translation_marker_matches(&self, marker: &Path) -> bool {
        if !self.client_extracted_dir().is_dir() {
            return false;
        }
        let Ok(text) = std::fs::read_to_string(marker) else {
            return false;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        let expected = self.client_extracted_dir().display().to_string();
        value
            .get("extracted")
            .and_then(|entry| entry.as_str())
            .map(|written| written.eq_ignore_ascii_case(&expected))
            .unwrap_or(false)
    }

    pub fn server_built(&self) -> bool {
        self.server_bin().is_file()
    }

    pub fn client_built(&self) -> bool {
        self.client_bin().is_file()
    }
}

/// Default client repository (native client replacing roBrowserLegacy).
pub fn default_client_repo() -> String {
    "https://github.com/pts1667/nostalro-client.git".into()
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
mod translation_marker_tests {
    use super::Settings;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("ro-sppack-{label}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn translation_markers_must_point_at_the_current_destination() {
        let install = temp_dir("translation-marker");
        let settings = Settings {
            install_dir: install.clone(),
            ..Settings::default()
        };
        fs::create_dir_all(settings.client_extracted_dir()).unwrap();

        // Marker from the old webroot layout: no `extracted` field -> stale.
        fs::write(
            settings.translation_pre_marker(),
            r#"{"webroot":"C:\\old\\webroot"}"#,
        )
        .unwrap();
        assert!(!settings.translation_pre_applied());

        // Marker written for the current destination -> applied.
        let marker = serde_json::json!({ "extracted": settings.client_extracted_dir().display().to_string() });
        fs::write(settings.translation_pre_marker(), marker.to_string()).unwrap();
        assert!(settings.translation_pre_applied());

        // Extraction folder gone -> stale again.
        fs::remove_dir_all(settings.client_extracted_dir()).unwrap();
        assert!(!settings.translation_pre_applied());
        fs::remove_dir_all(&install).ok();
    }
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
