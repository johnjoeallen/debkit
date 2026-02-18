use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

pub const DEFAULT_WALLPAPERS_FOLDER: &str = "/net/spitfire/data/share/jallen/wallpapers";
pub const DEFAULT_INTERVAL_MINUTES: u32 = 10;

#[derive(Debug, Clone)]
pub struct DebkitConfig {
    pub wallpapers: WallpapersConfig,
    pub variety: VarietyConfig,
    pub foundation: FoundationConfig,
}

impl Default for DebkitConfig {
    fn default() -> Self {
        Self {
            wallpapers: WallpapersConfig::default(),
            variety: VarietyConfig::default(),
            foundation: FoundationConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WallpapersConfig {
    pub folder: String,
}

impl Default for WallpapersConfig {
    fn default() -> Self {
        Self {
            folder: DEFAULT_WALLPAPERS_FOLDER.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VarietyConfig {
    pub interval_minutes: u32,
}

impl Default for VarietyConfig {
    fn default() -> Self {
        Self {
            interval_minutes: DEFAULT_INTERVAL_MINUTES,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FoundationConfig {
    pub install: Vec<String>,
}

pub fn load_or_init() -> anyhow::Result<DebkitConfig> {
    let home = home_dir()?;
    load_or_init_for_home(&home)
}

pub fn load_or_init_for_home(home: &Path) -> anyhow::Result<DebkitConfig> {
    let path = config_path_for_home(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if !path.exists() {
        let default_cfg = DebkitConfig::default();
        fs::write(&path, serialize_config(&default_cfg))
            .with_context(|| format!("failed to write {}", path.display()))?;
        return Ok(default_cfg);
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let (config, missing_keys) = parse_config(&raw)?;

    if missing_keys.any_missing() {
        fs::write(&path, serialize_config(&config))
            .with_context(|| format!("failed to update {}", path.display()))?;
    }

    if config.variety.interval_minutes == 0 {
        bail!("`variety.interval_minutes` must be greater than 0");
    }

    Ok(config)
}

pub fn config_path_for_home(home: &Path) -> PathBuf {
    home.join(".config").join("debkit").join("config.toml")
}

pub fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}

#[derive(Debug, Clone, Copy)]
struct MissingKeys {
    wallpapers_folder: bool,
    variety_interval_minutes: bool,
    foundation_install: bool,
}

impl MissingKeys {
    fn any_missing(self) -> bool {
        self.wallpapers_folder || self.variety_interval_minutes || self.foundation_install
    }
}

fn parse_config(raw: &str) -> anyhow::Result<(DebkitConfig, MissingKeys)> {
    let mut config = DebkitConfig::default();
    let mut section = String::new();

    let mut seen_wallpapers_folder = false;
    let mut seen_variety_interval = false;
    let mut seen_foundation_install = false;

    for (idx, line) in raw.lines().enumerate() {
        let stripped = strip_comment(line);
        let trimmed = stripped.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].trim().to_string();
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            bail!("invalid config line {}: expected `key = value`", idx + 1);
        };

        let key = key.trim();
        let value = value.trim();

        match (section.as_str(), key) {
            ("wallpapers", "folder") => {
                config.wallpapers.folder = parse_string_value(value).with_context(|| {
                    format!("invalid string at line {} for wallpapers.folder", idx + 1)
                })?;
                seen_wallpapers_folder = true;
            }
            ("variety", "interval_minutes") => {
                config.variety.interval_minutes = value.parse::<u32>().with_context(|| {
                    format!(
                        "invalid integer at line {} for variety.interval_minutes",
                        idx + 1
                    )
                })?;
                seen_variety_interval = true;
            }
            ("foundation", "install") => {
                config.foundation.install = parse_string_array(value).with_context(|| {
                    format!("invalid array at line {} for foundation.install", idx + 1)
                })?;
                seen_foundation_install = true;
            }
            _ => {}
        }
    }

    let missing = MissingKeys {
        wallpapers_folder: !seen_wallpapers_folder,
        variety_interval_minutes: !seen_variety_interval,
        foundation_install: !seen_foundation_install,
    };

    Ok((config, missing))
}

fn parse_string_value(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.starts_with('"') {
        if !value.ends_with('"') || value.len() < 2 {
            bail!("missing closing quote");
        }
        Ok(unescape_basic(&value[1..value.len() - 1]))
    } else {
        Ok(value.to_string())
    }
}

fn parse_string_array(value: &str) -> anyhow::Result<Vec<String>> {
    let value = value.trim();
    if !value.starts_with('[') || !value.ends_with(']') {
        bail!("array must use [..] format");
    }

    let inner = value[1..value.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    for part in inner.split(',') {
        items.push(parse_string_value(part.trim())?);
    }
    Ok(items)
}

fn strip_comment(line: &str) -> String {
    let mut out = String::new();
    let mut in_string = false;
    let chars = line.chars();

    for c in chars {
        if c == '"' {
            in_string = !in_string;
            out.push(c);
            continue;
        }

        if c == '#' && !in_string {
            break;
        }

        out.push(c);
    }

    out
}

fn serialize_config(config: &DebkitConfig) -> String {
    format!(
        "[wallpapers]\nfolder = \"{}\"\n\n[variety]\ninterval_minutes = {}\n\n[foundation]\ninstall = {}\n",
        escape_basic(&config.wallpapers.folder),
        config.variety.interval_minutes,
        serialize_array(&config.foundation.install)
    )
}

fn serialize_array(items: &[String]) -> String {
    let quoted = items
        .iter()
        .map(|item| format!("\"{}\"", escape_basic(item)))
        .collect::<Vec<_>>();
    format!("[{}]", quoted.join(", "))
}

fn escape_basic(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape_basic(raw: &str) -> String {
    raw.replace("\\\"", "\"").replace("\\\\", "\\")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn initializes_default_config() {
        let home = temp_home("default_init");
        let config = load_or_init_for_home(&home).unwrap();

        assert_eq!(config.wallpapers.folder, DEFAULT_WALLPAPERS_FOLDER);
        assert_eq!(config.variety.interval_minutes, DEFAULT_INTERVAL_MINUTES);

        let config_path = config_path_for_home(&home);
        assert!(config_path.exists());
    }

    #[test]
    fn backfills_missing_keys_without_overwriting_existing_values() {
        let home = temp_home("backfill");
        let config_path = config_path_for_home(&home);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            "[wallpapers]\nfolder = \"/tmp/walls\"\n[foundation]\n",
        )
        .unwrap();

        let config = load_or_init_for_home(&home).unwrap();
        assert_eq!(config.wallpapers.folder, "/tmp/walls");
        assert_eq!(config.variety.interval_minutes, DEFAULT_INTERVAL_MINUTES);
        assert!(config.foundation.install.is_empty());

        let rewritten = fs::read_to_string(config_path).unwrap();
        assert!(rewritten.contains("interval_minutes"));
        assert!(rewritten.contains("/tmp/walls"));
    }

    #[test]
    fn parses_foundation_install_array() {
        let raw = "[foundation]\ninstall = [\"variety\", \"rust\"]\n";
        let (config, missing) = parse_config(raw).unwrap();
        assert_eq!(config.foundation.install, vec!["variety", "rust"]);
        assert!(missing.wallpapers_folder);
        assert!(missing.variety_interval_minutes);
        assert!(!missing.foundation_install);
    }

    fn temp_home(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "debkit_test_config_{}_{}_{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
