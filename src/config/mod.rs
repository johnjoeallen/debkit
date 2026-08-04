mod schema;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde_yaml_ng::Value;

use schema::DebkitConfigFile;
pub use schema::{
    DEFAULT_ESSENTIAL_PACKAGES, DEFAULT_REBOOT_MODE, DebkitConfig, DnsConfig, EssentialsConfig,
    GitConfig, LinkEntry, NisConfig, SudoNopassConfig, WakeOnLanConfig,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnableSectionResult {
    pub path: PathBuf,
    pub changed: bool,
}

/// Sets `debkit.<section>.enabled: true` in the per-user base config
/// (`~/.config/debkit/config.yaml`, i.e. `config_path_for_home`) — the target for
/// `debkit enable <module>`. `section` is a raw config-schema key (e.g.
/// `"hardware_grub"`), already resolved from a module's dotted name by the caller
/// (see `Module::config_key`) — this function doesn't know about modules at all, only
/// the raw YAML shape, matching `add_nis_slave_to_raw_config`'s layering. Creates the
/// file (and its parent directory) if it doesn't exist yet, and creates the section
/// mapping if the file exists but doesn't mention it. Like `add_nis_slave_to_host`,
/// re-serializes the whole parsed document rather than patching text in place, so
/// pre-existing comments in the file are not preserved.
pub fn enable_section_for_home(home: &Path, section: &str) -> anyhow::Result<EnableSectionResult> {
    let path = config_path_for_home(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let raw = if path.exists() {
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };
    let mut value = parse_yaml(&raw)?;
    let debkit = ensure_mapping_key(&mut value, "debkit");
    let section_map = ensure_mapping_key(debkit, section);
    let enabled_key = Value::String("enabled".to_string());

    if matches!(
        section_map.as_mapping().and_then(|m| m.get(&enabled_key)),
        Some(Value::Bool(true))
    ) {
        return Ok(EnableSectionResult {
            path,
            changed: false,
        });
    }

    section_map
        .as_mapping_mut()
        .expect("ensure_mapping_key returns a mapping")
        .insert(enabled_key, Value::Bool(true));

    let rendered =
        serde_yaml_ng::to_string(&value).context("failed to serialize updated config")?;
    fs::write(&path, rendered).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(EnableSectionResult {
        path,
        changed: true,
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

/// The `/etc/debkit/config.yaml` global tier: packaged by the `.deb` (as a dpkg
/// conffile, from `config.example.yaml`) and the base of the merge chain below. DebKit
/// is mostly not a per-user tool -- most module config (hardware, network, identity,
/// apt, pam, systemd) describes the machine, not the operator running it -- so this is
/// where that belongs. It's purely optional at read time: a missing file is "no
/// global overrides," never an error, exactly like a missing host overlay today.
/// Deliberately never auto-created by DebKit itself (see `load_or_init_for_home`'s doc
/// comment) -- provisioning it is the package's job.
const GLOBAL_CONFIG_PATH: &str = "/etc/debkit/config.yaml";

/// Unlike `configure_complete_for_home` (the explicit, opt-in `debkit host-config`
/// path, which still writes a full default `config.yaml` on request), this no longer
/// auto-writes one as a side effect of a passive read. It used to: before the global
/// tier existed, an implicitly-created full-default per-user file was harmless. Now
/// that `/etc/debkit/config.yaml` is packaged and present on a normal install, writing
/// a complete per-user dump the first time *any* command ran would immediately shadow
/// every key in the global file for that user, forever -- defeating the global tier
/// entirely. A missing `~/.config/debkit/config.yaml` is simply "no per-user
/// overrides," same as a missing host overlay; every field still has an in-code
/// default either way.
pub fn load_or_init_for_home(home: &Path) -> anyhow::Result<DebkitConfig> {
    let hostname = current_hostname().unwrap_or_else(|_| schema::DEFAULT_HOST_NAME.to_string());

    let merged = merged_value_for_home_and_hostname(home, &hostname)?;
    let mut config = deserialize_config(merged)?;

    config.host.name = hostname.clone();
    if config.wake_on_lan.reference_host.trim().is_empty() {
        config.wake_on_lan.reference_host = hostname;
    }

    validate_config(&config)?;
    Ok(config)
}

/// Deep-merges three optional YAML tiers, lowest to highest priority: the global
/// `/etc/debkit/config.yaml`, the per-user base config, then the per-user host
/// overlay. Any key present in a higher tier wins; anything it doesn't mention falls
/// through to the tier below untouched. This replaces v1's hand-maintained
/// `MissingKeys`/`apply_host_overlay` pair with one generic merge.
fn merged_value_for_home_and_hostname(home: &Path, hostname: &str) -> anyhow::Result<Value> {
    merged_value(Path::new(GLOBAL_CONFIG_PATH), home, hostname)
}

fn merged_value(global_path: &Path, home: &Path, hostname: &str) -> anyhow::Result<Value> {
    let mut merged = Value::Mapping(Default::default());
    merged = merge_optional_yaml(merged, global_path)?;
    merged = merge_optional_yaml(merged, &config_path_for_home(home))?;
    merged = merge_optional_yaml(merged, &host_config_path_for_home(home, hostname))?;
    Ok(merged)
}

/// Merges `path`'s parsed YAML on top of `base` if it exists; otherwise returns `base`
/// unchanged. A missing file at any tier is a normal "nothing declared here," never an
/// error -- only an existing-but-unreadable-or-invalid file is.
fn merge_optional_yaml(base: Value, path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(base);
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(merge_values(base, parse_yaml(&raw)?))
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

/// The config-owning user's home directory -- almost always what a caller actually
/// wants, since DebKit's config lives under `~/.config/debkit/`. Deliberately NOT
/// just `$HOME`: `sudo` resets `HOME` to the target user's home (`/root` by default,
/// per Debian's `env_reset`/`always_set_home` sudoers defaults), so `sudo debkit
/// apply ...` run by a real user would otherwise silently read `/root/.config/...`
/// (normally absent, falling back to all-defaults config) instead of that user's
/// actual declared config -- exactly the failure mode that makes an apply look
/// "already compliant" when it isn't. When running as root via a real `sudo`
/// invocation (`SUDO_USER` set), resolve the invoking user's home from
/// `/etc/passwd` instead. Root logged in directly (no `SUDO_USER`) still uses
/// `$HOME` as-is.
pub fn home_dir() -> anyhow::Result<PathBuf> {
    if crate::engine::exec::current_euid().ok() == Some(0)
        && let Some(sudo_user) = std::env::var_os("SUDO_USER")
    {
        let sudo_user = sudo_user.to_string_lossy().trim().to_string();
        if !sudo_user.is_empty() && sudo_user != "root" {
            if let Some(home) = passwd_home_for_user(&sudo_user) {
                return Ok(home);
            }
            return Ok(PathBuf::from(format!("/home/{sudo_user}")));
        }
    }

    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}

fn passwd_home_for_user(user: &str) -> Option<PathBuf> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd_home_for_user_from_passwd(user, &passwd)
}

fn passwd_home_for_user_from_passwd(user: &str, passwd: &str) -> Option<PathBuf> {
    for line in passwd.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split(':');
        if fields.next()? != user {
            continue;
        }
        let home = fields.nth(4)?; // passwd, uid, gid, gecos, then home
        return Some(PathBuf::from(home));
    }
    None
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
    if !matches!(
        config.hardware_grub.reboot_mode.as_str(),
        "" | "cold" | "warm"
    ) {
        bail!("`hardware_grub.reboot_mode` must be one of `cold`, `warm`, or empty");
    }
    if !matches!(
        config.hardware_grub.reboot_type.as_str(),
        "" | "bios" | "acpi" | "kbd" | "triple" | "efi" | "pci"
    ) {
        bail!(
            "`hardware_grub.reboot_type` must be one of `bios`, `acpi`, `kbd`, `triple`, `efi`, `pci`, or empty"
        );
    }
    if !is_valid_video_mode(config.hardware_grub.video_mode.as_str()) {
        bail!("`hardware_grub.video_mode` must be `<columns>x<rows>` (e.g. `1024x768`) or empty");
    }
    Ok(())
}

/// `video_mode` sets GRUB_GFXMODE/GRUB_GFXPAYLOAD_LINUX in /etc/default/grub (see
/// modules::hardware_grub), in the simple `<columns>x<rows>` form rather than a raw
/// resolution/depth mode number -- both components must be plain positive-integer
/// pixel counts. Same shell-injection reasoning as `reboot_mode`/`reboot_type`: this
/// value is written into a file `update-grub` sources as a shell script.
fn is_valid_video_mode(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let Some((columns, rows)) = value.split_once('x') else {
        return false;
    };
    is_positive_pixel_count(columns) && is_positive_pixel_count(rows)
}

fn is_positive_pixel_count(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|c| c.is_ascii_digit())
        && value.parse::<u32>().is_ok_and(|n| n > 0)
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
    fn passwd_home_lookup_finds_the_matching_user() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\njallen:x:1000:1000:John Allen,,,:/home/jallen:/bin/bash\n";
        assert_eq!(
            passwd_home_for_user_from_passwd("jallen", passwd),
            Some(PathBuf::from("/home/jallen"))
        );
    }

    #[test]
    fn passwd_home_lookup_returns_none_for_an_unknown_user() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n";
        assert_eq!(
            passwd_home_for_user_from_passwd("nobody-real", passwd),
            None
        );
    }

    #[test]
    fn passwd_home_lookup_skips_comments_and_blank_lines() {
        let passwd = "# comment\n\njallen:x:1000:1000::/home/jallen:/bin/bash\n";
        assert_eq!(
            passwd_home_for_user_from_passwd("jallen", passwd),
            Some(PathBuf::from("/home/jallen"))
        );
    }

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

        // Unlike v1 (and unlike this project's own pre-global-config-tier
        // behavior), a passive read no longer auto-writes a config.yaml -- doing so
        // would permanently shadow the global /etc/debkit/config.yaml tier for this
        // user. Every field still has an in-code default, so a fully absent config
        // tree is a normal, supported state.
        let config_path = config_path_for_home(&home);
        assert!(!config_path.exists());
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
    fn global_config_supplies_a_value_when_user_and_host_files_are_absent() {
        let home = temp_home("global_only");
        let hostname = current_hostname().unwrap();
        let global_dir = temp_home("global_only_etc");
        let global_path = global_dir.join("config.yaml");
        fs::write(
            &global_path,
            "debkit:\n  hardware_grub:\n    enabled: true\n    reboot_mode: cold\n",
        )
        .unwrap();

        let value = merged_value(&global_path, &home, &hostname).unwrap();
        let config = deserialize_config(value).unwrap();
        assert!(config.hardware_grub.enabled);
        assert_eq!(config.hardware_grub.reboot_mode, "cold");
    }

    #[test]
    fn user_config_overrides_a_conflicting_global_key() {
        let home = temp_home("global_vs_user");
        let hostname = current_hostname().unwrap();
        let global_dir = temp_home("global_vs_user_etc");
        let global_path = global_dir.join("config.yaml");
        fs::write(
            &global_path,
            "debkit:\n  hardware_grub:\n    enabled: true\n    reboot_mode: cold\n",
        )
        .unwrap();

        let base_path = config_path_for_home(&home);
        fs::create_dir_all(base_path.parent().unwrap()).unwrap();
        fs::write(
            &base_path,
            "debkit:\n  hardware_grub:\n    reboot_mode: warm\n",
        )
        .unwrap();

        let value = merged_value(&global_path, &home, &hostname).unwrap();
        let config = deserialize_config(value).unwrap();
        // The user's own file wins on the conflicting key...
        assert_eq!(config.hardware_grub.reboot_mode, "warm");
        // ...but a global-only key the user file never mentions still comes through.
        assert!(config.hardware_grub.enabled);
    }

    #[test]
    fn host_overlay_overrides_both_global_and_user_config() {
        let home = temp_home("global_vs_user_vs_host");
        let hostname = current_hostname().unwrap();
        let global_dir = temp_home("global_vs_user_vs_host_etc");
        let global_path = global_dir.join("config.yaml");
        fs::write(
            &global_path,
            "debkit:\n  hardware_grub:\n    reboot_mode: cold\n",
        )
        .unwrap();

        let base_path = config_path_for_home(&home);
        fs::create_dir_all(base_path.parent().unwrap()).unwrap();
        fs::write(
            &base_path,
            "debkit:\n  hardware_grub:\n    reboot_mode: warm\n",
        )
        .unwrap();

        let host_path = host_config_path_for_home(&home, &hostname);
        fs::create_dir_all(host_path.parent().unwrap()).unwrap();
        fs::write(
            &host_path,
            "debkit:\n  hardware_grub:\n    reboot_mode: \"\"\n",
        )
        .unwrap();

        let value = merged_value(&global_path, &home, &hostname).unwrap();
        let config = deserialize_config(value).unwrap();
        assert_eq!(config.hardware_grub.reboot_mode, "");
    }

    #[test]
    fn missing_global_config_is_not_an_error_and_leaves_defaults_in_place() {
        let home = temp_home("no_global");
        let hostname = current_hostname().unwrap();
        let nonexistent_global = temp_home("no_global_etc").join("does-not-exist.yaml");

        let value = merged_value(&nonexistent_global, &home, &hostname).unwrap();
        let config = deserialize_config(value).unwrap();
        assert!(!config.hardware_grub.enabled);
    }

    #[test]
    fn enable_section_creates_the_file_and_section_when_absent() {
        let home = temp_home("enable_fresh");
        let result = enable_section_for_home(&home, "hardware_grub").unwrap();
        assert!(result.changed);
        assert_eq!(result.path, config_path_for_home(&home));

        let config = load_or_init_for_home(&home).unwrap();
        assert!(config.hardware_grub.enabled);
    }

    #[test]
    fn enable_section_preserves_other_sections_and_fields() {
        let home = temp_home("enable_preserve");
        let config_path = config_path_for_home(&home);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            "debkit:\n  hardware_grub:\n    reboot_mode: cold\n  wake_on_lan:\n    enabled: true\n",
        )
        .unwrap();

        let result = enable_section_for_home(&home, "hardware_grub").unwrap();
        assert!(result.changed);

        let config = load_or_init_for_home(&home).unwrap();
        assert!(config.hardware_grub.enabled);
        assert_eq!(config.hardware_grub.reboot_mode, "cold");
        assert!(config.wake_on_lan.enabled);
    }

    #[test]
    fn enable_section_is_idempotent() {
        let home = temp_home("enable_idempotent");
        enable_section_for_home(&home, "hardware_grub").unwrap();
        let result = enable_section_for_home(&home, "hardware_grub").unwrap();
        assert!(!result.changed);
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

    #[test]
    fn video_mode_accepts_empty_and_columns_x_rows() {
        assert!(is_valid_video_mode(""));
        assert!(is_valid_video_mode("1024x768"));
        assert!(is_valid_video_mode("1920x1080"));
        assert!(is_valid_video_mode("640x480"));
    }

    #[test]
    fn video_mode_rejects_zero_missing_or_non_numeric_components() {
        assert!(!is_valid_video_mode("0x768"));
        assert!(!is_valid_video_mode("1024x0"));
        assert!(!is_valid_video_mode("1024x"));
        assert!(!is_valid_video_mode("x768"));
        assert!(!is_valid_video_mode("1024"));
        assert!(!is_valid_video_mode("1024x768x32"));
        assert!(!is_valid_video_mode("1024.5x768"));
    }

    #[test]
    fn video_mode_rejects_shell_metacharacters_and_legacy_vga_words() {
        assert!(!is_valid_video_mode("1024x768; rm -rf /"));
        assert!(!is_valid_video_mode("$(reboot)"));
        // The old vga= vocabulary no longer applies now that video_mode drives
        // GRUB_GFXMODE/GRUB_GFXPAYLOAD_LINUX instead of the kernel's vga= parameter.
        assert!(!is_valid_video_mode("ask"));
        assert!(!is_valid_video_mode("791"));
        assert!(!is_valid_video_mode("0x317"));
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
