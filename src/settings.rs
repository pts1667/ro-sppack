//! Persisted launcher settings and the paths derived from them.

use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

/// Accepted range for every experience and drop rate (a multiplier, `1.0` is the default).
pub const RATE_RANGE: RangeInclusive<f64> = 0.0..=10000.0;
/// Accepted range for mob density, the number of mobs per spawn point.
pub const MOB_DENSITY_RANGE: RangeInclusive<f64> = 0.0..=10.0;
/// Accepted range for the packet version, written as `YYYYMMDD`.
pub const PACKETVER_RANGE: RangeInclusive<f64> = 1.0..=99_999_999.0;
/// Accepted range for every TCP port.
pub const PORT_RANGE: RangeInclusive<f64> = 1.0..=65535.0;
/// Accepted range for the bots playing at once. Each takes turns with a resting character, and
/// rust-ro-bot's `names.txt` holds 640 names.
pub const BOT_COUNT_RANGE: RangeInclusive<f64> = 1.0..=300.0;

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
    pub drop_rate_mvp: f32,
    pub mob_density: f32,

    /// Port of the server's client data endpoint (`/data`), the client's `data_url`.
    pub client_data_port: u16,
    /// Bot API (`/bots`), which lets an external program play characters.
    pub bot_api_enabled: bool,
    pub bot_api_port: u16,
    /// Port of the server's read-only reference (`/db`, `/wiki`), which the bots plan with.
    pub reference_port: u16,
    /// Logins from this computer skip the account system (`login.skip_account_ips`): the client
    /// asks only for a name, and an unknown name gets an account on first use.
    pub ip_login: bool,
    /// Start rust-ro-bot once the server accepts connections.
    pub launch_bots: bool,
    /// Bots playing at once (`--bots`).
    pub bot_count: u32,

    pub build_release: bool,
    /// "Play without updating": the launcher does not pull and rebuild when it starts. An install
    /// that is not complete is still finished.
    pub skip_update: bool,
    /// Start the game server with `--debug-log` (script failure diagnostics).
    pub debug_log: bool,

    pub server_repo: String,
    pub client_repo: String,
    pub bot_repo: String,

    /// Copy the ROenglishRE translation (renewal base, pre-renewal overlay and
    /// the classic client compatibility tables) over the client's loose data.
    pub install_translation: bool,
    pub translation_repo: String,
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
            drop_rate_mvp: 1.0,
            mob_density: 1.0,
            client_data_port: 6903,
            bot_api_enabled: true,
            bot_api_port: 6902,
            reference_port: 6904,
            ip_login: true,
            launch_bots: true,
            bot_count: 50,
            build_release: true,
            skip_update: false,
            debug_log: false,
            server_repo: "https://github.com/pts1667/rust-ro.git".into(),
            client_repo: default_client_repo(),
            bot_repo: "https://github.com/pts1667/rust-ro-bot.git".into(),
            install_translation: system_language_is_english(),
            translation_repo: default_translation_repo(),
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
        // The translation moved from the two zackdreaver repositories (renewal
        // base + pre-renewal overlay) to llchrisll/ROenglishRE, which ships the
        // renewal base, the pre-renewal files and old-client compatibility
        // tables in one clone.
        if self.translation_repo.trim().is_empty() || self.translation_repo.contains("zackdreaver")
        {
            self.translation_repo = default_translation_repo();
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

    pub fn bot_dir(&self) -> PathBuf {
        self.install_dir.join("rust-ro-bot")
    }

    pub fn bot_bin(&self) -> PathBuf {
        self.bot_dir()
            .join("target")
            .join(self.profile())
            .join(format!("rust-ro-bot{}", std::env::consts::EXE_SUFFIX))
    }

    /// The Bot API key, which the server writes on its first start (`bots.api_key_path`).
    pub fn bot_api_key_file(&self) -> PathBuf {
        self.server_dir().join("config").join("bots_api_key.txt")
    }

    /// `rust-ro-bot run` with its defaults (the decision model as the bots' voice, characters taking
    /// turns). `--server-dir` reads the Bot API and reference addresses and the API key from the
    /// server's config.json; `--stop-on-stdin` lets the launcher log them out with a line `stop`.
    pub fn bot_args(&self) -> Vec<String> {
        vec![
            "run".into(),
            "--bots".into(),
            self.bot_count.to_string(),
            "--server-dir".into(),
            self.server_dir().display().to_string(),
            "--stop-on-stdin".into(),
        ]
    }

    /// Loose translation files the client reads before the server's data (the client's `data/`).
    pub fn client_data_dir(&self) -> PathBuf {
        self.client_dir().join("data")
    }

    /// Loose files that override the server's client data (`data_dir` in config.json).
    pub fn client_extracted_dir(&self) -> PathBuf {
        self.client_data_dir().join("extracted")
    }

    /// The tree the server publishes under `/data`, built by `client-data-setup` in rust-ro.
    pub fn server_client_data_dir(&self) -> PathBuf {
        self.server_dir().join("config").join("client-data")
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

    pub fn translation_dir(&self) -> PathBuf {
        self.install_dir.join("ROenglishRE")
    }

    /// Layer roots inside the clone, applied in this order. The pre-renewal
    /// files overwrite the renewal base, and the 2012-04-10 compatibility
    /// files (the classic `*item*table.txt` files a 2012-era client reads)
    /// win last. Mirrors `Tools/ClientGenerator.bat` for Pre-Renewal.
    pub fn translation_sources(&self) -> Vec<PathBuf> {
        const LAYERS: [&str; 3] = [
            "Translation/Renewal",
            "Translation/Pre-Renewal",
            "Translation/Compatibility/2012-04-10/Pre-Renewal",
        ];
        LAYERS
            .iter()
            .map(|layer| self.translation_dir().join(layer))
            .collect()
    }

    /// Marker written once the translation has been copied into the client data.
    pub fn translation_marker(&self) -> PathBuf {
        self.install_dir.join("translation.json")
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

    /// Base URL of the client data endpoint as the client should reach it (`data_url`).
    pub fn client_data_url(&self) -> String {
        let host = self.connect_host();
        // IPv6 literals are bracketed inside a URL.
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host
        };
        format!("http://{host}:{}/data", self.client_data_port)
    }

    /// Address the client should use to reach the game server.
    /// Addresses whose logins skip the account system: loopback, and the address the client
    /// connects to when it is an IPv4 address of this machine (LAN play), since the client then
    /// connects from it. Empty without `ip_login`.
    pub fn ip_login_addresses(&self) -> Vec<String> {
        if !self.ip_login {
            return Vec::new();
        }
        let mut addresses = vec!["127.0.0.1".to_string()];
        let connect = self.connect_host();
        if connect.parse::<std::net::Ipv4Addr>().is_ok() && !addresses.contains(&connect) {
            addresses.push(connect);
        }
        addresses
    }

    pub fn connect_host(&self) -> String {
        let public = self.public_host.trim();
        if !public.is_empty() {
            return public.to_string();
        }
        self.server_probe_host()
    }

    /// Address the launcher checks to see whether the server accepts connections on this
    /// machine: the bind host, with the wildcard addresses mapped to loopback.
    pub fn server_probe_host(&self) -> String {
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

    pub fn bot_cloned(&self) -> bool {
        self.bot_dir().join("Cargo.toml").is_file()
    }

    /// All layers of the clone must be present, so a leftover clone of the old
    /// zackdreaver translation is not mistaken for a complete one.
    pub fn translation_cloned(&self) -> bool {
        self.translation_sources()
            .iter()
            .all(|source| source.join("data").is_dir())
    }

    /// True once the translation was copied over the client data.
    pub fn translation_applied(&self) -> bool {
        self.translation_marker_matches(&self.translation_marker())
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

    /// Checks every number the menu lets the user type, so an out-of-range value never reaches a
    /// generated config file (for example a hand-edited `ro-sppack.json`).
    pub fn validate(&self) -> Result<(), String> {
        let rates = [
            ("Base EXP rate", self.base_exp_rate),
            ("Job EXP rate", self.job_exp_rate),
            ("Drop rate", self.drop_rate),
            ("Card drop rate", self.drop_rate_card),
            ("MVP drop rate", self.drop_rate_mvp),
        ];
        for (name, value) in rates {
            check_range(name, value, &RATE_RANGE)?;
        }
        check_range("Mob density", self.mob_density, &MOB_DENSITY_RANGE)?;
        check_range("Packet version", self.packetver, &PACKETVER_RANGE)?;
        check_range("Bot count", self.bot_count, &BOT_COUNT_RANGE)?;

        let mut ports = vec![
            ("Game port", self.game_port),
            ("Client data port", self.client_data_port),
            ("Reference port", self.reference_port),
        ];
        if self.bot_api_enabled {
            ports.push(("Bot API port", self.bot_api_port));
        }
        for (name, port) in &ports {
            check_range(name, *port, &PORT_RANGE)?;
        }
        for (index, (name, port)) in ports.iter().enumerate() {
            if let Some((other, _)) = ports[..index].iter().find(|(_, earlier)| earlier == port) {
                return Err(format!(
                    "{name} and {other} are both {port}; choose different ports"
                ));
            }
        }
        Ok(())
    }

    pub fn client_built(&self) -> bool {
        self.client_bin().is_file()
    }

    pub fn bot_built(&self) -> bool {
        self.bot_bin().is_file()
    }

    /// Ports the server opens once it is initialized. The launcher waits for all of them before it
    /// starts the client and the bots.
    pub fn server_ports(&self) -> Vec<u16> {
        let mut ports = vec![self.game_port, self.client_data_port, self.reference_port];
        if self.bot_api_enabled {
            ports.push(self.bot_api_port);
        }
        ports
    }
}

/// `value` must be a finite number inside `range`.
fn check_range<T: Copy + Into<f64>>(
    name: &str,
    value: T,
    range: &RangeInclusive<f64>,
) -> Result<(), String> {
    let number: f64 = value.into();
    if number.is_finite() && range.contains(&number) {
        Ok(())
    } else {
        Err(format!(
            "{name} must be between {} and {}",
            range.start(),
            range.end()
        ))
    }
}

/// Default client repository (native client replacing roBrowserLegacy).
pub fn default_client_repo() -> String {
    "https://github.com/pts1667/nostalro-client.git".into()
}

/// Default translation repository (renewal base + pre-renewal + compatibility).
pub fn default_translation_repo() -> String {
    "https://github.com/llchrisll/ROenglishRE.git".into()
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
            settings.translation_marker(),
            r#"{"webroot":"C:\\old\\webroot"}"#,
        )
        .unwrap();
        assert!(!settings.translation_applied());

        // Marker written for the current destination -> applied.
        let marker = serde_json::json!({ "extracted": settings.client_extracted_dir().display().to_string() });
        fs::write(settings.translation_marker(), marker.to_string()).unwrap();
        assert!(settings.translation_applied());

        // Extraction folder gone -> stale again.
        fs::remove_dir_all(settings.client_extracted_dir()).unwrap();
        assert!(!settings.translation_applied());
        fs::remove_dir_all(&install).ok();
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn legacy_translation_repositories_are_replaced() {
        let mut settings: Settings = serde_json::from_str(
            r#"{
                "translation_repo": "https://github.com/zackdreaver/ROenglishRE.git",
                "translation_re_repo": "https://github.com/zackdreaver/ROenglishRE.git",
                "translation_pre_repo": "https://github.com/zackdreaver/ROenglishPRE.git"
            }"#,
        )
        .unwrap();
        settings.migrate();
        assert_eq!(settings.translation_repo, default_translation_repo());

        // A custom repository is kept as is.
        let mut custom: Settings =
            serde_json::from_str(r#"{"translation_repo":"https://example.com/my/fork.git"}"#)
                .unwrap();
        custom.migrate();
        assert_eq!(custom.translation_repo, "https://example.com/my/fork.git");
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

#[cfg(test)]
mod bot_tests {
    use super::Settings;

    #[test]
    fn logins_from_this_computer_skip_the_account_system() {
        let mut settings = Settings::default();
        assert!(settings.ip_login);
        assert_eq!(settings.ip_login_addresses(), ["127.0.0.1"]);

        // LAN play: the client connects to (and so from) the machine's LAN address.
        settings.public_host = "192.168.1.20".into();
        assert_eq!(settings.ip_login_addresses(), ["127.0.0.1", "192.168.1.20"]);
        // A host name is no address pattern the server accepts.
        settings.public_host = "my-pc.local".into();
        assert_eq!(settings.ip_login_addresses(), ["127.0.0.1"]);

        settings.ip_login = false;
        assert!(settings.ip_login_addresses().is_empty());
    }

    #[test]
    fn bots_read_the_server_config_and_stop_on_stdin() {
        let settings = Settings {
            bot_count: 12,
            bot_api_port: 7002,
            reference_port: 7004,
            ..Settings::default()
        };
        let args = settings.bot_args();
        assert_eq!(args[..3], ["run", "--bots", "12"]);
        let index = args.iter().position(|arg| arg == "--server-dir").unwrap();
        assert_eq!(args[index + 1], settings.server_dir().display().to_string());
        assert!(args.iter().any(|arg| arg == "--stop-on-stdin"));
        assert!(settings.server_ports().contains(&7002));
        assert!(settings.server_ports().contains(&7004));
    }

    #[test]
    fn settings_files_from_before_the_bots_launch_fifty() {
        let settings: Settings = serde_json::from_str(r#"{"game_port": 7000}"#).unwrap();
        assert!(settings.launch_bots);
        assert_eq!(settings.bot_count, 50);
        assert_eq!(settings.reference_port, 6904);
    }
}
