//! Generates the server configuration and patches the native client's
//! `config.json`.
//!
//! The client owns its configuration file: it reads it on start and writes it
//! back on quit and when settings change in game. The launcher therefore only
//! updates the connection and resource fields and preserves everything else
//! (window state, keybindings, volumes, ...).

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
    settings.validate()?;
    create_directories(settings)?;
    generate_server_config(settings)
}

/// Patches the connection and resource fields of the client `config.json`.
pub fn generate_client(settings: &Settings) -> Result<(), String> {
    settings.validate()?;
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
        config["game"]["drop_rate_mvp"] = json!(settings.drop_rate_mvp);
        config["game"]["mob_density"] = json!(settings.mob_density);
    }

    // Logins from this computer need no account: the client asks only for a name.
    config["login"]["skip_account_ips"] = json!(settings.ip_login_addresses());

    // The server publishes the client data it extracted from the kRO archives. It listens on the
    // same bind host as the game, and the client is pointed at it through `data_url`.
    config["client_data"]["enabled"] = json!(true);
    config["client_data"]["host"] = json!(settings.server_host);
    config["client_data"]["port"] = json!(settings.client_data_port);
    config["bots"]["enabled"] = json!(settings.bot_api_enabled);
    config["bots"]["port"] = json!(settings.bot_api_port);
    // The bots plan their trips with the reference's `/db`.
    config["reference"]["enabled"] = json!(true);
    config["reference"]["port"] = json!(settings.reference_port);
    if settings.bot_api_enabled {
        // The bots take turns with a pool of twice their number (`--pool`): every character of
        // the pool must fit in `max_registered_bots`, and `max_bots` leaves room to spare.
        let room = settings.bot_count as u64 * 2;
        for key in ["max_bots", "max_registered_bots"] {
            let current = config["bots"][key].as_u64().unwrap_or(0);
            config["bots"][key] = json!(current.max(room));
        }
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

    // Every resource comes from the server's `/data` endpoint. The client resolves data_dir first
    // (the translation, when applied), then data_url, then grf_paths. Archives copied by earlier
    // launcher versions are no longer listed, and the BGM tracks come from `bgm/` on the endpoint.
    config["data_url"] = json!(settings.client_data_url());
    config["grf_paths"] = json!([]);
    if let Some(object) = config.as_object_mut() {
        object.remove("bgm_path");
    }
    if settings.client_extracted_dir().is_dir() {
        config["data_dir"] = json!("data/extracted");
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

    const SERVER_TEMPLATE: &str = r#"{
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
            "drop_rate_mvp": 1.0,
            "mob_density": 1.0
        },
        "login": { "new_account": true, "skip_account_ips": [] },
        "bots": { "enabled": false, "host": "127.0.0.1", "port": 6902, "max_bots": 20, "max_registered_bots": 200 },
        "reference": { "enabled": true, "host": "127.0.0.1", "port": 6904, "wiki_root": "wiki" },
        "client_data": { "enabled": false, "host": "127.0.0.1", "port": 6903, "root": "config/client-data" }
    }"#;

    /// An install with a cloned rust-ro (template only) and a client whose config carries
    /// fields the launcher does not own.
    fn install(label: &str) -> (std::path::PathBuf, Settings) {
        let root = temp_dir(label);
        let server = root.join("rust-ro");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(server.join("config.template.json"), SERVER_TEMPLATE).unwrap();
        // The client folder is cloned; an old copy-era config still lists archives and BGM.
        let client = root.join("nostalro-client");
        std::fs::create_dir_all(client.join("data").join("extracted")).unwrap();
        std::fs::write(
            client.join("config.json"),
            r#"{"dpi_scale": 150.0, "bgm_path": "data/BGM", "grf_paths": ["data/data.grf"]}"#,
        )
        .unwrap();
        let settings = Settings {
            install_dir: root.clone(),
            server_host: "0.0.0.0".into(),
            game_port: 7001,
            client_data_port: 7003,
            packetver: 20120307,
            base_exp_rate: 100.0,
            drop_rate_mvp: 5.0,
            ..Settings::default()
        };
        (root, settings)
    }

    #[test]
    fn generates_patched_server_and_client_configs() {
        let (root, settings) = install("patched");
        generate_all(&settings).unwrap();

        let server = settings.server_dir();
        let written = std::fs::read_to_string(server.join("config.json")).unwrap();
        let config: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(config["server"]["port"], json!(7001));
        assert_eq!(config["server"]["packetver"], json!(20120307));
        assert_eq!(config["server"]["enable_legacy_proxy"], json!(false));
        assert_eq!(config["server"]["enable_websocket"], json!(true));
        assert_eq!(config["server"]["accounts"][0], json!(2_000_000));
        assert_eq!(config["game"]["base_exp_rate"], json!(100.0));
        assert_eq!(config["game"]["drop_rate_mvp"], json!(5.0));
        assert_eq!(config["client_data"]["enabled"], json!(true));
        assert_eq!(config["client_data"]["host"], json!("0.0.0.0"));
        assert_eq!(config["client_data"]["port"], json!(7003));
        assert_eq!(config["client_data"]["root"], json!("config/client-data"));
        assert_eq!(config["bots"]["enabled"], json!(true));
        assert_eq!(config["bots"]["port"], json!(6902));
        // 50 bots by default: room for 100 in game, and the template's 200 registered is kept.
        assert_eq!(config["bots"]["max_bots"], json!(100));
        assert_eq!(config["bots"]["max_registered_bots"], json!(200));
        assert_eq!(config["reference"]["enabled"], json!(true));
        assert_eq!(config["reference"]["port"], json!(6904));
        assert_eq!(config["reference"]["wiki_root"], json!("wiki"));
        assert_eq!(config["login"]["skip_account_ips"], json!(["127.0.0.1"]));
        assert_eq!(config["login"]["new_account"], json!(true));

        let written = std::fs::read_to_string(settings.client_config_json()).unwrap();
        let config: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(config["packetver"], json!(20120307));
        assert_eq!(config["login_servers"][0]["host"], json!("127.0.0.1"));
        assert_eq!(config["login_servers"][0]["port"], json!(7001));
        assert_eq!(config["login_servers"][0]["packetver"], json!(20120307));
        assert_eq!(config["data_url"], json!("http://127.0.0.1:7003/data"));
        assert_eq!(config["grf_paths"], json!([]));
        assert!(config.get("bgm_path").is_none());
        assert_eq!(config["data_dir"], json!("data/extracted"));
        // Untouched fields written by the client are preserved.
        assert_eq!(config["dpi_scale"], json!(150.0));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn invalid_numbers_are_not_written() {
        let (root, mut settings) = install("invalid");
        settings.base_exp_rate = 10000.5;
        assert!(generate_server(&settings).is_err());
        assert!(!settings.server_config_json().exists());

        settings.base_exp_rate = f32::NAN;
        assert!(generate_server(&settings).is_err());

        settings.base_exp_rate = 10000.0;
        settings.client_data_port = settings.game_port;
        let before = std::fs::read_to_string(settings.client_config_json()).unwrap();
        let error = generate_client(&settings).unwrap_err();
        assert!(error.contains("different ports"), "{error}");
        assert_eq!(
            std::fs::read_to_string(settings.client_config_json()).unwrap(),
            before
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
