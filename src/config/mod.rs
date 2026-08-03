mod schema;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde_yaml_ng::Value;

use schema::DebkitConfigFile;
pub use schema::{
    DEFAULT_ESSENTIAL_PACKAGES, DebkitConfig, DnsConfig, EssentialsConfig, GitConfig, LinkEntry,
    NisConfig, SudoNopassConfig, WakeOnLanConfig,
};

pub fn load_or_init() -> anyhow::Result<DebkitConfig> {
    let home = home_dir()?;
    load_or_init_for_home(&home)
}

pub fn configure_complete_for_current_host() -> anyhow::Result<PathBuf> {
    let home = home_dir()?;
    configure_complete_for_home(&home)
}

pub fn add_nis_slave_to_host(master_host: &str, slave: &str) -> anyhow::Result<AddNisSlaveResult> {
    let home = home_dir()?;
    add_nis_slave_to_host_for_home(&home, master_host, slave)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddNisSlaveResult {
    pub path: PathBuf,
    pub added: bool,
}

pub fn add_nis_slave_to_host_for_home(
    home: &Path,
    master_host: &str,
    slave: &str,
) -> anyhow::Result<AddNisSlaveResult> {
    let slave = slave.trim();
    if slave.is_empty() {
        bail!("slave hostname must not be empty");
    }

    let host_path = host_config_path_for_home(home, master_host);
    if !host_path.exists() {
        bail!(
            "host config {} does not exist; run `debkit host-config` on that host or create it first",
            host_path.display()
        );
    }

    let config = load_for_home_and_hostname(home, master_host)?;
    if !config.nis.enabled || config.nis.role != "master" {
        bail!("host `{master_host}` must have `nis.enabled = true` and `nis.role = \"master\"`");
    }

    let raw = fs::read_to_string(&host_path)
        .with_context(|| format!("failed to read {}", host_path.display()))?;
    let (updated, added) = add_nis_slave_to_raw_config(&raw, slave)?;
    if added {
        fs::write(&host_path, updated)
            .with_context(|| format!("failed to write {}", host_path.display()))?;
    }
    Ok(AddNisSlaveResult {
        path: host_path,
        added,
    })
}

fn load_for_home_and_hostname(home: &Path, hostname: &str) -> anyhow::Result<DebkitConfig> {
    let merged = merged_value_for_home_and_hostname(home, hostname)?;
    let mut config = deserialize_config(merged)?;
    config.host.name = hostname.to_string();
    validate_config(&config)?;
    Ok(config)
}

pub fn configure_complete_for_home(home: &Path) -> anyhow::Result<PathBuf> {
    let hostname = current_hostname().unwrap_or_else(|_| schema::DEFAULT_HOST_NAME.to_string());
    let base_path = config_path_for_home(home);
    if let Some(parent) = base_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if !base_path.exists() {
        let default_cfg = DebkitConfig::for_hostname(&hostname);
        fs::write(&base_path, serialize_config(&default_cfg)?)
            .with_context(|| format!("failed to write {}", base_path.display()))?;
    }

    let path = host_config_path_for_home(home, &hostname);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if !path.exists() {
        let content = format!(
            "# DebKit host overrides for {hostname}\n# This file supplements ~/.config/debkit/config.yaml.\n# Add only values that differ for this host, e.g.:\n#\n# debkit:\n#   wake_on_lan:\n#     enabled: false\n"
        );
        fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(path)
}

pub fn load_or_init_for_home(home: &Path) -> anyhow::Result<DebkitConfig> {
    let hostname = current_hostname().unwrap_or_else(|_| schema::DEFAULT_HOST_NAME.to_string());
    let path = config_path_for_home(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if !path.exists() {
        let default_cfg = DebkitConfig::for_hostname(&hostname);
        fs::write(&path, serialize_config(&default_cfg)?)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    let merged = merged_value_for_home_and_hostname(home, &hostname)?;
    let mut config = deserialize_config(merged)?;

    config.host.name = hostname.clone();
    if config.wake_on_lan.reference_host.trim().is_empty() {
        config.wake_on_lan.reference_host = hostname;
    }

    validate_config(&config)?;
    Ok(config)
}

/// Loads the base config as a `serde_yaml` value and, if a host override exists, deep-merges
/// it on top. Any key present in the host file wins; anything the host file doesn't mention
/// falls through to the base value untouched. This replaces v1's hand-maintained
/// `MissingKeys`/`apply_host_overlay` pair with one generic merge.
fn merged_value_for_home_and_hostname(home: &Path, hostname: &str) -> anyhow::Result<Value> {
    let base_path = config_path_for_home(home);
    let base_raw = fs::read_to_string(&base_path)
        .with_context(|| format!("failed to read {}", base_path.display()))?;
    let mut merged = parse_yaml(&base_raw)?;

    let host_path = host_config_path_for_home(home, hostname);
    if host_path.exists() {
        let host_raw = fs::read_to_string(&host_path)
            .with_context(|| format!("failed to read {}", host_path.display()))?;
        let overlay = parse_yaml(&host_raw)?;
        merged = merge_values(merged, overlay);
    }

    Ok(merged)
}

fn deserialize_config(value: Value) -> anyhow::Result<DebkitConfig> {
    let file: DebkitConfigFile =
        serde_yaml_ng::from_value(value).context("invalid DebKit YAML config")?;
    Ok(file.debkit)
}

fn parse_yaml(raw: &str) -> anyhow::Result<Value> {
    if raw.trim().is_empty() {
        return Ok(Value::Mapping(Default::default()));
    }
    serde_yaml_ng::from_str(raw).context("invalid YAML")
}

fn merge_values(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Mapping(mut base_map), Value::Mapping(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let merged = match base_map.remove(&key) {
                    Some(base_val) => merge_values(base_val, overlay_val),
                    None => overlay_val,
                };
                base_map.insert(key, merged);
            }
            Value::Mapping(base_map)
        }
        (_, overlay_value) => overlay_value,
    }
}

pub fn config_path_for_home(home: &Path) -> PathBuf {
    home.join(".config").join("debkit").join("config.yaml")
}

pub fn host_config_path_for_home(home: &Path, hostname: &str) -> PathBuf {
    home.join(".config")
        .join("debkit")
        .join("hosts")
        .join(format!("{}.yaml", sanitize_hostname_for_path(hostname)))
}

pub fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}

fn validate_config(config: &DebkitConfig) -> anyhow::Result<()> {
    if config.variety.interval_minutes == 0 {
        bail!("`variety.interval_minutes` must be greater than 0");
    }
    if config.npm.version.trim().is_empty() {
        bail!("`npm.version` must not be empty");
    }
    if config.sudo_nopass.group.trim().is_empty() {
        bail!("`sudo_nopass.group` must not be empty");
    }
    if config.nis.enabled && config.nis.domain.trim().is_empty() {
        bail!("`nis.domain` must be set when `nis.enabled = true`");
    }
    if config.nis.enabled && config.nis.admin_user.trim().is_empty() {
        bail!("`nis.admin_user` must be set when `nis.enabled = true`");
    }
    if config.nis.enabled && !matches!(config.nis.role.as_str(), "master" | "slave" | "client") {
        bail!("`nis.role` must be one of `master`, `slave`, or `client`");
    }
    if config.nis.enabled && config.nis.role == "slave" && config.nis.master.trim().is_empty() {
        bail!("`nis.master` must be set when `nis.role = \"slave\"`");
    }
    if config.nis.enabled
        && config.nis.role == "client"
        && config.nis.server.trim().is_empty()
        && config.nis.servers.is_empty()
    {
        bail!("`nis.server` must be set when `nis.role = \"client\"`");
    }
    if config.wake_on_lan.enabled && config.wake_on_lan.mode != "magic" {
        bail!("`wake_on_lan.mode` currently supports only `magic`");
    }
    if !matches!(
        config.wake_on_lan.backend.as_str(),
        "auto" | "network_manager" | "networkmanager" | "ethtool"
    ) {
        bail!("`wake_on_lan.backend` must be one of `network_manager`, `ethtool`, or `auto`");
    }
    if config.git.enabled && config.git.credential_helper.trim().is_empty() {
        bail!("`git.credential_helper` must not be empty when `git.enabled = true`");
    }
    if !matches!(
        config.hardware_sleep.desired_mem_sleep.as_str(),
        "" | "s2idle" | "deep"
    ) {
        bail!("`hardware_sleep.desired_mem_sleep` must be one of `s2idle`, `deep`, or empty");
    }
    Ok(())
}

fn current_hostname() -> anyhow::Result<String> {
    let raw = std::process::Command::new("hostname")
        .output()
        .context("failed to run hostname")?;
    if !raw.status.success() {
        bail!("hostname failed with status {}", raw.status);
    }
    Ok(String::from_utf8_lossy(&raw.stdout).trim().to_string())
}

fn sanitize_hostname_for_path(hostname: &str) -> String {
    let sanitized = hostname
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        schema::DEFAULT_HOST_NAME.to_string()
    } else {
        sanitized
    }
}

fn serialize_config(config: &DebkitConfig) -> anyhow::Result<String> {
    let file = DebkitConfigFile {
        debkit: config.clone(),
    };
    serde_yaml_ng::to_string(&file).context("failed to serialize DebKit config")
}

fn add_nis_slave_to_raw_config(raw: &str, slave: &str) -> anyhow::Result<(String, bool)> {
    let mut value = parse_yaml(raw)?;
    let debkit = ensure_mapping_key(&mut value, "debkit");
    let nis = ensure_mapping_key(debkit, "nis");
    let slaves_key = Value::String("slaves".to_string());

    let mut slaves = match nis.as_mapping().and_then(|m| m.get(&slaves_key)) {
        Some(Value::Sequence(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    if slaves.iter().any(|existing| existing == slave) {
        return Ok((raw.to_string(), false));
    }
    slaves.push(slave.to_string());

    let sequence = Value::Sequence(slaves.into_iter().map(Value::String).collect());
    nis.as_mapping_mut()
        .expect("ensure_mapping_key returns a mapping")
        .insert(slaves_key, sequence);

    let rendered =
        serde_yaml_ng::to_string(&value).context("failed to serialize updated config")?;
    Ok((rendered, true))
}

/// Ensures `parent[key]` is a mapping (creating it if absent or replacing a non-mapping
/// value) and returns a mutable reference to it.
fn ensure_mapping_key<'a>(parent: &'a mut Value, key: &str) -> &'a mut Value {
    if !matches!(parent, Value::Mapping(_)) {
        *parent = Value::Mapping(Default::default());
    }
    let map = parent.as_mapping_mut().expect("just ensured mapping");
    let key_value = Value::String(key.to_string());
    if !matches!(map.get(&key_value), Some(Value::Mapping(_))) {
        map.insert(key_value.clone(), Value::Mapping(Default::default()));
    }
    map.get_mut(&key_value).expect("key was just inserted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_default_config() {
        let home = temp_home("default_init");
        let config = load_or_init_for_home(&home).unwrap();

        assert_ne!(config.host.name, schema::DEFAULT_HOST_NAME);
        assert_eq!(config.wallpapers.folder, schema::DEFAULT_WALLPAPERS_FOLDER);
        assert_eq!(
            config.variety.interval_minutes,
            schema::DEFAULT_INTERVAL_MINUTES
        );
        assert_eq!(
            config.foundation.install,
            vec![
                "essentials",
                "git",
                "ripgrep",
                "rust",
                "npm",
                "codex",
                "variety",
                "nis",
                "wake-on-lan"
            ]
        );
        assert_eq!(
            config.essentials.packages,
            schema::DEFAULT_ESSENTIAL_PACKAGES.to_vec()
        );
        assert_eq!(config.npm.version, schema::DEFAULT_NPM_VERSION);
        assert!(!config.sudo_nopass.enabled);
        assert!(!config.nis.enabled);
        assert_eq!(config.nis.role, schema::DEFAULT_NIS_ROLE);
        assert!(config.wake_on_lan.enabled);
        assert_eq!(config.wake_on_lan.reference_host, config.host.name);
        assert!(config.wake_on_lan.interfaces_auto);

        let config_path = config_path_for_home(&home);
        assert!(config_path.exists());
    }

    #[test]
    fn partial_base_config_backfills_defaults_without_rewriting_the_file() {
        let home = temp_home("backfill");
        let config_path = config_path_for_home(&home);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            "debkit:\n  wallpapers:\n    folder: /tmp/walls\n",
        )
        .unwrap();

        let config = load_or_init_for_home(&home).unwrap();
        assert_eq!(config.wallpapers.folder, "/tmp/walls");
        assert_eq!(
            config.variety.interval_minutes,
            schema::DEFAULT_INTERVAL_MINUTES
        );
        assert_eq!(config.npm.version, schema::DEFAULT_NPM_VERSION);
        assert!(!config.sudo_nopass.enabled);

        // Unlike v1, defaults are applied purely in-memory; the sparse file on disk is
        // left untouched.
        let raw = fs::read_to_string(&config_path).unwrap();
        assert_eq!(raw, "debkit:\n  wallpapers:\n    folder: /tmp/walls\n");
    }

    #[test]
    fn configures_complete_host_config() {
        let home = temp_home("complete_host");
        let path = configure_complete_for_home(&home).unwrap();
        let hostname = current_hostname().unwrap();
        assert_eq!(path, host_config_path_for_home(&home, &hostname));
        let raw = fs::read_to_string(path).unwrap();
        assert!(raw.contains(&format!("DebKit host overrides for {hostname}")));
        assert!(raw.contains("supplements ~/.config/debkit/config.yaml"));

        let base_raw = fs::read_to_string(config_path_for_home(&home)).unwrap();
        assert!(base_raw.contains("wake_on_lan:"));
    }

    #[test]
    fn host_config_overlay_supplements_base_config() {
        let home = temp_home("host_overlay");
        let hostname = current_hostname().unwrap();
        let base_path = config_path_for_home(&home);
        fs::create_dir_all(base_path.parent().unwrap()).unwrap();
        fs::write(
            &base_path,
            "debkit:\n  foundation:\n    install: [git]\n  wake_on_lan:\n    backend: network_manager\n",
        )
        .unwrap();

        let path = host_config_path_for_home(&home, &hostname);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "debkit:\n  wake_on_lan:\n    enabled: true\n").unwrap();

        let config = load_or_init_for_home(&home).unwrap();
        assert_eq!(config.foundation.install, vec!["git"]);
        assert_eq!(config.host.name, hostname);
        assert!(config.wake_on_lan.enabled);
        assert_eq!(config.wake_on_lan.backend, "network_manager");
    }

    #[test]
    fn host_config_path_uses_sanitized_hostname() {
        let home = PathBuf::from("/tmp/home");
        assert_eq!(
            host_config_path_for_home(&home, "bad/name"),
            PathBuf::from("/tmp/home/.config/debkit/hosts/bad_name.yaml")
        );
    }

    #[test]
    fn parses_nis_config() {
        let raw = "debkit:\n  nis:\n    enabled: true\n    role: master\n    domain: example.internal\n    admin_user: admin\n    local_admin_groups: [sudo, wheel]\n    master: iris.example.internal\n    server: iris.example.internal\n    prefer_local: false\n    push_to_slaves: true\n    force_refresh_maps: true\n    slaves: [spitfire.example.internal, laptop.example.internal]\n    servers: [legacy1, legacy2]\n";
        let value = parse_yaml(raw).unwrap();
        let config = deserialize_config(value).unwrap();
        assert!(config.nis.enabled);
        assert_eq!(config.nis.role, "master");
        assert_eq!(config.nis.domain, "example.internal");
        assert_eq!(config.nis.admin_user, "admin");
        assert_eq!(config.nis.local_admin_groups, vec!["sudo", "wheel"]);
        assert!(!config.nis.prefer_local);
        assert!(config.nis.push_to_slaves);
        assert!(config.nis.force_refresh_maps);
        assert_eq!(
            config.nis.slaves,
            vec!["spitfire.example.internal", "laptop.example.internal"]
        );
        assert_eq!(config.nis.servers, vec!["legacy1", "legacy2"]);
    }

    #[test]
    fn add_nis_slave_updates_existing_slaves_array() {
        let raw = "debkit:\n  nis:\n    enabled: true\n    role: master\n    slaves: [node-a.example.lan]\n";
        let (updated, added) = add_nis_slave_to_raw_config(raw, "node-b.example.lan").unwrap();
        assert!(added);
        let value = parse_yaml(&updated).unwrap();
        let config = deserialize_config(value).unwrap();
        assert_eq!(
            config.nis.slaves,
            vec!["node-a.example.lan", "node-b.example.lan"]
        );
    }

    #[test]
    fn add_nis_slave_is_idempotent() {
        let raw = "debkit:\n  nis:\n    slaves: [node-a.example.lan]\n";
        let (updated, added) = add_nis_slave_to_raw_config(raw, "node-a.example.lan").unwrap();
        assert!(!added);
        assert_eq!(updated, raw);
    }

    #[test]
    fn add_nis_slave_inserts_missing_slaves_key() {
        let raw = "debkit:\n  nis:\n    enabled: true\n    role: master\n  wake_on_lan:\n    enabled: true\n";
        let (updated, added) = add_nis_slave_to_raw_config(raw, "node-a.example.lan").unwrap();
        assert!(added);
        let value = parse_yaml(&updated).unwrap();
        let config = deserialize_config(value).unwrap();
        assert_eq!(config.nis.slaves, vec!["node-a.example.lan"]);
        assert!(config.wake_on_lan.enabled);
    }

    fn temp_home(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "debkit_v2_test_config_{}_{}_{}",
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
