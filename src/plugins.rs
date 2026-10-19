//! The game server's plugins (rust-ro `docs/plugins.md`): the ones installed in its plugins folder,
//! what each declares, and the choices the launcher writes for them.
//!
//! A plugin is a folder `<plugins_dir>/<name>/` with a `plugin.wasm`. Its `info.json` (title,
//! description, settings with their defaults) is compiled into the module as text, so the launcher
//! finds it in the module's bytes instead of running the plugin. The server loads the plugins
//! listed and enabled in `scripting.plugins`, and reads a plugin's settings from
//! `<plugins_dir>/<name>.json`, which holds only the keys that differ from the defaults.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The `plugins_dir` of a server without one in its configuration.
const DEFAULT_PLUGINS_DIR: &str = "config/plugins";

/// What the launcher sets for one plugin, kept in `ro-sppack.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginChoice {
    /// Loaded or not; `None` keeps what the server's template says.
    pub enabled: Option<bool>,
    /// The settings that differ from the plugin's defaults.
    pub settings: Map<String, Value>,
}

/// The `info.json` of a plugin.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub config: BTreeMap<String, Setting>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    /// Its value when the operator does not set it. A value of another JSON type is refused,
    /// unless the default is `null`.
    pub default: Value,
    #[serde(default)]
    pub description: String,
}

/// A plugin installed in the plugins folder.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    /// Its `info.json`, or why it could not be read.
    pub manifest: Result<Manifest, String>,
    /// Whether the server's template loads it.
    pub enabled_by_template: bool,
    /// Its `<name>.json` as found, which the launcher keeps until its settings are changed here.
    pub saved: Map<String, Value>,
}

/// The plugins folder of a server configuration, relative to the server's folder.
pub fn plugins_dir(server_dir: &Path, config: &Value) -> PathBuf {
    server_dir.join(
        config["scripting"]["plugins_dir"]
            .as_str()
            .unwrap_or(DEFAULT_PLUGINS_DIR),
    )
}

/// The plugins installed for a server configuration (its template), sorted by name.
pub fn discover(server_dir: &Path, config: &Value) -> Result<Vec<Plugin>, String> {
    let dir = plugins_dir(server_dir, config);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", dir.display())),
    };
    let mut plugins: Vec<Plugin> = entries
        .flatten()
        .filter(|entry| entry.path().join("plugin.wasm").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .map(|name| Plugin {
            manifest: read_manifest(&dir, &name),
            enabled_by_template: listed(config, &name).is_some_and(entry_enabled),
            saved: std::fs::read(dir.join(format!("{name}.json")))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default(),
            name,
        })
        .collect();
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(plugins)
}

/// The `info.json` compiled into `<dir>/<name>/plugin.wasm`.
pub fn read_manifest(dir: &Path, name: &str) -> Result<Manifest, String> {
    let path = dir.join(name).join("plugin.wasm");
    let bytes =
        std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    manifest_in_module(&bytes).ok_or_else(|| {
        "its info.json was not found in plugin.wasm (built with an older SDK?)".into()
    })
}

/// Finds the `info.json` text in a module's data: a JSON object starting with a `"title"` key that
/// decodes as a manifest.
fn manifest_in_module(bytes: &[u8]) -> Option<Manifest> {
    const KEY: &[u8] = b"\"title\"";
    let mut from = 0;
    while let Some(found) = bytes[from..]
        .windows(KEY.len())
        .position(|window| window == KEY)
    {
        let key = from + found;
        from = key + 1;
        // Back over the whitespace before the key to the object's opening brace.
        let Some(brace) = bytes[..key]
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .filter(|&index| bytes[index] == b'{')
        else {
            continue;
        };
        let mut stream =
            serde_json::Deserializer::from_slice(&bytes[brace..]).into_iter::<Manifest>();
        if let Some(Ok(manifest)) = stream.next()
            && !manifest.title.trim().is_empty()
        {
            return Some(manifest);
        }
    }
    None
}

/// Why `value` cannot be the setting `key` of a plugin, if it cannot.
pub fn check_setting(manifest: &Manifest, key: &str, value: &Value) -> Result<(), String> {
    let Some(setting) = manifest.config.get(key) else {
        return Err(format!("{key} is no longer a setting of this plugin"));
    };
    if !setting.default.is_null() && json_type(value) != json_type(&setting.default) {
        return Err(format!(
            "{key} must be {}, not {}",
            json_type(&setting.default),
            json_type(value)
        ));
    }
    Ok(())
}

pub fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The entry of `scripting.plugins` for `name`.
fn listed<'a>(config: &'a Value, name: &str) -> Option<&'a Value> {
    config["scripting"]["plugins"]
        .as_array()?
        .iter()
        .find(|entry| entry.as_str().or_else(|| entry["name"].as_str()) == Some(name))
}

/// A name alone is enabled; an object is unless it says `"enabled": false`.
fn entry_enabled(entry: &Value) -> bool {
    entry.is_string() || entry["enabled"].as_bool().unwrap_or(true)
}

/// Writes the launcher's choices into a server configuration and the plugins folder: an enabled
/// plugin is listed (after the template's, which keep their order and capabilities), a disabled one
/// stays listed but off, and each plugin the launcher has settings for gets its `<name>.json`, or
/// none when every setting is the default.
pub fn apply(
    server_dir: &Path,
    config: &mut Value,
    choices: &BTreeMap<String, PluginChoice>,
) -> Result<(), String> {
    let dir = plugins_dir(server_dir, config);
    for (name, choice) in choices {
        let installed = dir.join(name).join("plugin.wasm").is_file();
        if !installed {
            // A plugin removed by an update: listing it would stop the boot.
            continue;
        }
        if let Some(enabled) = choice.enabled {
            set_enabled(config, name, enabled);
        }
        let path = dir.join(format!("{name}.json"));
        if choice.settings.is_empty() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("remove {}: {error}", path.display())),
            }
            continue;
        }
        if listed(config, name).is_some_and(entry_enabled) {
            // The server refuses a plugin whose settings it does not know; say which one here.
            let manifest =
                read_manifest(&dir, name).map_err(|error| format!("Plugin {name}: {error}"))?;
            for (key, value) in &choice.settings {
                check_setting(&manifest, key, value).map_err(|error| {
                    format!("Plugin {name}: {error}; reset it in Settings → Plugins")
                })?;
            }
        }
        let text = serde_json::to_string_pretty(&choice.settings)
            .map_err(|error| format!("serialize {}: {error}", path.display()))?;
        std::fs::write(&path, text)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    Ok(())
}

fn set_enabled(config: &mut Value, name: &str, enabled: bool) {
    if !config["scripting"]["plugins"].is_array() {
        config["scripting"]["plugins"] = json!([]);
    }
    let Some(list) = config["scripting"]["plugins"].as_array_mut() else {
        return;
    };
    match list
        .iter_mut()
        .find(|entry| entry.as_str().or_else(|| entry["name"].as_str()) == Some(name))
    {
        Some(entry) if entry.is_object() => entry["enabled"] = json!(enabled),
        Some(entry) => *entry = json!({ "name": name, "enabled": enabled }),
        None => list.push(json!({ "name": name, "enabled": enabled })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = r#"{
  "title": "Curio Trader",
  "description": "A shop.",
  "config": {
    "title": { "default": "Curio Trader", "description": "Title of the shop window." },
    "max_amount": { "default": 100, "description": "Most items bought at once." },
    "stock": { "default": [{ "item_id": 501, "price": 40 }], "description": "Items on sale." }
  }
}"#;

    /// A module whose data holds other strings around the manifest, as the compiler lays them out.
    fn module() -> Vec<u8> {
        let mut bytes = b"\0asm\x01\0\0\0 struct Settings".to_vec();
        bytes.extend_from_slice(INFO.as_bytes());
        bytes.extend_from_slice(b"\0\0called `Option::unwrap()`{\"title\": 3}");
        bytes
    }

    fn temp_server(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("ro-sppack-plugins-{label}-{unique}"));
        for name in ["ui_shop", "double_xp"] {
            let folder = dir.join("config/plugins").join(name);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("plugin.wasm"), module()).unwrap();
        }
        // A folder without a module is no plugin.
        std::fs::create_dir_all(dir.join("config/plugins/notes")).unwrap();
        std::fs::write(
            dir.join("config/plugins/ui_shop.json"),
            r#"{"max_amount": 5}"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn the_manifest_is_read_from_the_module() {
        let manifest = manifest_in_module(&module()).unwrap();
        assert_eq!(manifest.title, "Curio Trader");
        assert_eq!(manifest.config["max_amount"].default, json!(100));
        assert_eq!(manifest.config["stock"].description, "Items on sale.");
        assert!(manifest_in_module(b"\0asm{\"title\": 3}").is_none());
    }

    #[test]
    fn installed_plugins_are_found_with_the_templates_choice() {
        let server = temp_server("discover");
        let config = json!({ "scripting": { "plugins": [
            "ui_shop",
            { "name": "double_xp", "enabled": false }
        ] } });
        let plugins = discover(&server, &config).unwrap();
        let names: Vec<_> = plugins.iter().map(|plugin| plugin.name.as_str()).collect();
        assert_eq!(names, ["double_xp", "ui_shop"]);
        assert!(!plugins[0].enabled_by_template);
        assert!(plugins[1].enabled_by_template);
        assert_eq!(plugins[1].manifest.as_ref().unwrap().title, "Curio Trader");
        assert!(plugins[0].saved.is_empty());
        assert_eq!(plugins[1].saved["max_amount"], json!(5));
        std::fs::remove_dir_all(&server).ok();
    }

    #[test]
    fn choices_reach_the_plugin_list_and_settings_files() {
        let server = temp_server("apply");
        let dir = server.join("config/plugins");
        std::fs::write(dir.join("double_xp.json"), r#"{"exp_percent": 300}"#).unwrap();
        let mut config = json!({ "scripting": { "plugins": [
            { "name": "double_xp", "enabled": false, "capabilities": ["Rates"] },
            "ui_shop"
        ] } });
        let mut choices = BTreeMap::new();
        choices.insert(
            "double_xp".to_string(),
            PluginChoice {
                enabled: Some(true),
                settings: Map::new(),
            },
        );
        let mut shop = Map::new();
        shop.insert("max_amount".into(), json!(5));
        choices.insert(
            "ui_shop".to_string(),
            PluginChoice {
                enabled: Some(false),
                settings: shop,
            },
        );
        // Not installed: left out of the list.
        choices.insert(
            "gone".to_string(),
            PluginChoice {
                enabled: Some(true),
                settings: Map::new(),
            },
        );
        apply(&server, &mut config, &choices).unwrap();
        assert_eq!(
            config["scripting"]["plugins"],
            json!([
                { "name": "double_xp", "enabled": true, "capabilities": ["Rates"] },
                { "name": "ui_shop", "enabled": false }
            ])
        );
        // Only defaults: no settings file.
        assert!(!dir.join("double_xp.json").exists());
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("ui_shop.json")).unwrap())
                .unwrap();
        assert_eq!(written, json!({ "max_amount": 5 }));
        std::fs::remove_dir_all(&server).ok();
    }

    #[test]
    fn an_enabled_plugin_with_a_wrong_setting_is_not_written() {
        let server = temp_server("wrong");
        let mut config = json!({ "scripting": { "plugins": [] } });
        let mut settings = Map::new();
        settings.insert("max_amount".into(), json!("lots"));
        let mut choices = BTreeMap::new();
        choices.insert(
            "ui_shop".to_string(),
            PluginChoice {
                enabled: Some(true),
                settings,
            },
        );
        let error = apply(&server, &mut config, &choices).unwrap_err();
        assert!(error.contains("max_amount must be a number"), "{error}");
        std::fs::remove_dir_all(&server).ok();
    }
}
