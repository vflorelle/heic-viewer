//! Options (langue de l'interface), enregistrées en TOML.
//!
//! Emplacement : `heic-viewer.toml` à côté de l'exécutable s'il existe (mode portable),
//! sinon `%APPDATA%\heic-viewer\config.toml` (Windows) ou `~/.config/heic-viewer/config.toml`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub display: Display,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
    pub language: Language,
}

#[derive(Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "en-US")]
    English,
    #[serde(rename = "fr-FR")]
    French,
}

impl Config {
    /// Configuration enregistrée, ou par défaut, et l'erreur de lecture éventuelle.
    pub fn load() -> (Config, Option<String>) {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(config) => (config, None),
                Err(e) => {
                    let path = path.display();
                    let error = tr!(
                        "Configuration illisible ({path}) : {e}",
                        "Unreadable configuration ({path}): {e}"
                    );
                    (Config::default(), Some(error))
                }
            },
            Err(_) => (Config::default(), None),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path();
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                let dir = dir.display();
                tr!("{dir} : {e}", "{dir}: {e}")
            })?;
        }
        std::fs::write(&path, text).map_err(|e| {
            let path = path.display();
            tr!("{path} : {e}", "{path}: {e}")
        })
    }
}

fn config_path() -> PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        let portable = dir.join("heic-viewer.toml");
        if portable.is_file() {
            return portable;
        }
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.unwrap_or_default().join("heic-viewer").join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut config = Config::default();
        config.display.language = Language::English;
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("language = \"en-US\""));
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.display.language, Language::English);
        // Les champs absents prennent leur valeur par défaut, les inconnus sont ignorés.
        let partial: Config = toml::from_str("ancien = 1\n[display]\n").unwrap();
        assert_eq!(partial.display.language, Language::Auto);
    }
}
