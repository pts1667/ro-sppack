//! Generates the server configuration and patches the native client's
//! `config.json`.
//!
//! The client owns its configuration file: it reads it on start and writes it
//! back on quit and when settings change in game. The launcher therefore only
//! updates the connection and resource fields and preserves everything else
//! (window state, keybindings, volumes, ...).

use crate::assets;
use crate::settings::Settings;

use serde_json::{Value, json};

/// Regenerate every file the launcher owns. Safe to call before each launch.
pub fn generate_all(settings: &Settings) -> Result<String, String> {
    generate_server(settings)?;
    generate_client(settings)?;
    Ok(format!(
        "configuration written: {}, {}",
        settings.server_config_json().display(),
        settings.client_config_json().display()
    ))
}

/// Writes only `rust-ro/config.json`.
pub fn generate_server(settings: &Settings) -> Result<(), String> {
    create_directories(settings)?;
    generate_server_config(settings)
}

/// Patches the connection and resource fields of the client `config.json`.
pub fn generate_client(settings: &Settings) -> Result<(), String> {
    create_directories(settings)?;
    generate_client_config(settings)
}

fn create_directories(settings: &Settings) -> Result<(), String> {
    for dir in [settings.client_data_dir(), settings.server_dir()] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    Ok(())
}

fn generate_server_config(settings: &Settings) -> Result<(), String> {
    let template_path = settings.server_dir().join("config.template.json");
    let text = std::fs::read_to_string(&template_path)
        .map_err(|e| format!("read {} (is rust-ro cloned?): {e}", template_path.display()))?;
    let mut config: Value = serde_json::from_str(&text)
        .map_err(|e| format!("parse {}: {e}", template_path.display()))?;

    let server = config
        .get_mut("server")
        .ok_or_else(|| "config.template.json has no `server` section".to_string())?;
    server["host"] = json!(settings.server_host);
    server["port"] = json!(settings.game_port);
    server["packetver"] = json!(settings.packetver);
    server["enable_websocket"] = json!(true);
    // Only needed when proxying real TCP clients to rAthena/Hercules.
    server["enable_legacy_proxy"] = json!(false);
    server["enable_visual_debugger"] = json!(false);

    match server
        .get_mut("accounts")
        .and_then(|value| value.as_array_mut())
    {
        Some(accounts) => {
            if !accounts
                .iter()
                .any(|value| value.as_u64() == Some(2_000_000))
            {
                accounts.push(json!(2_000_000));
            }
        }
        None => server["accounts"] = json!([2_000_000]),
    }

    if config.get("game").is_some() {
        config["game"]["base_exp_rate"] = json!(settings.base_exp_rate);
        config["game"]["job_exp_rate"] = json!(settings.job_exp_rate);
        config["game"]["drop_rate"] = json!(settings.drop_rate);
        config["game"]["drop_rate_card"] = json!(settings.drop_rate_card);
        config["game"]["mob_density"] = json!(settings.mob_density);
    }

    let config_path = settings.server_config_json();
    let pretty =
        serde_json::to_string_pretty(&config).map_err(|e| format!("serialize config: {e}"))?;
    std::fs::write(&config_path, pretty)
        .map_err(|e| format!("write {}: {e}", config_path.display()))
}

fn generate_client_config(settings: &Settings) -> Result<(), String> {
    let path = settings.client_config_json();
    let mut config: Value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    };
    if !config.is_object() {
        config = json!({});
    }

    config["packetver"] = json!(settings.packetver);
    config["login_servers"] = json!([{
        "name": "Local",
        "host": settings.connect_host(),
        "port": settings.game_port,
        "packetver": settings.packetver,
    }]);

    // The client resolves data_dir first, then grf_paths in order.
    let grfs = assets::grf_paths(settings);
    if !grfs.is_empty() {
        config["grf_paths"] = json!(grfs);
    }
    if settings.client_extracted_dir().is_dir() {
        config["data_dir"] = json!("data/extracted");
    }
    if settings.copy_bgm_ai && settings.client_bgm_dir().is_dir() {
        config["bgm_path"] = json!("data/BGM");
    } else {
        let bgm = settings.assets_dir.join("BGM");
        if bgm.is_dir() {
            config["bgm_path"] = json!(bgm.display().to_string());
        }
    }
    if config.get("emblem_path").is_none() {
        config["emblem_path"] = json!("data/emblem");
    }

    let pretty =
        serde_json::to_string_pretty(&config).map_err(|e| format!("serialize config: {e}"))?;
    std::fs::write(&path, pretty).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("ro-sppack-config-{label}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn generates_patched_server_and_client_configs() {
        let install = temp_dir("install");
        let server = install.join("rust-ro");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(
            server.join("config.template.json"),
            r#"{
                "server": {
                    "accounts": [],
                    "host": "0.0.0.0",
                    "port": 6901,
                    "packetver": 20120229,
                    "enable_legacy_proxy": true,
                    "enable_websocket": true,
                    "enable_visual_debugger": false,
                    "log_level": "info",
                    "trace_packet": false,
                    "log_level_module_override": []
                },
                "game": {
                    "base_exp_rate": 1.0,
                    "job_exp_rate": 1.0,
                    "drop_rate": 1.0,
                    "drop_rate_card": 1.0,
                    "mob_density": 1.0
                }
            }"#,
        )
        .unwrap();

        // Assets manifest + extracted dir as prepared by the asset step.
        let client = install.join("nostalro-client");
        let client_data = client.join("data");
        std::fs::create_dir_all(client_data.join("extracted")).unwrap();
        std::fs::write(client_data.join("data.grf"), b"grf").unwrap();
        std::fs::write(
            client_data.join("ro-sppack-assets.json"),
            r#"{"grf_paths": ["data/data.grf"]}"#,
        )
        .unwrap();
        // A previous run's settings that must survive the patch.
        std::fs::write(
            client.join("config.json"),
            r#"{"dpi_scale": 150.0, "bgm_path": "data/BGM"}"#,
        )
        .unwrap();

        let assets = install.join("assets");
        std::fs::create_dir_all(assets.join("BGM")).unwrap();

        let settings = Settings {
            install_dir: install.clone(),
            assets_dir: assets.clone(),
            server_host: "0.0.0.0".into(),
            game_port: 7001,
            packetver: 20120307,
            base_exp_rate: 100.0,
            ..Settings::default()
        };

        generate_all(&settings).unwrap();

        let written = std::fs::read_to_string(server.join("config.json")).unwrap();
        let config: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(config["server"]["port"], json!(7001));
        assert_eq!(config["server"]["packetver"], json!(20120307));
        assert_eq!(config["server"]["enable_legacy_proxy"], json!(false));
        assert_eq!(config["server"]["enable_websocket"], json!(true));
        assert_eq!(config["server"]["accounts"][0], json!(2_000_000));
        assert_eq!(config["game"]["base_exp_rate"], json!(100.0));

        let written = std::fs::read_to_string(client.join("config.json")).unwrap();
        let config: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(config["packetver"], json!(20120307));
        assert_eq!(config["login_servers"][0]["host"], json!("127.0.0.1"));
        assert_eq!(config["login_servers"][0]["port"], json!(7001));
        assert_eq!(config["login_servers"][0]["packetver"], json!(20120307));
        assert_eq!(config["grf_paths"][0], json!("data/data.grf"));
        assert_eq!(config["data_dir"], json!("data/extracted"));
        assert_eq!(
            config["bgm_path"],
            json!(assets.join("BGM").display().to_string())
        );
        // Untouched fields written by the client are preserved.
        assert_eq!(config["dpi_scale"], json!(150.0));

        std::fs::remove_dir_all(&install).ok();
    }
}
