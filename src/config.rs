use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

pub const DEFAULT_WALLPAPERS_FOLDER: &str = "";
pub const DEFAULT_INTERVAL_MINUTES: u32 = 10;
pub const DEFAULT_FOUNDATION_INSTALL: &[&str] = &[
    "essentials",
    "git",
    "ripgrep",
    "rust",
    "npm",
    "codex",
    "variety",
    "nis",
    "wake-on-lan",
];
pub const DEFAULT_ESSENTIAL_PACKAGES: &[&str] = &[
    "curl",
    "wget",
    "zip",
    "unzip",
    "rsync",
    "ca-certificates",
    "gnupg",
    "apt-transport-https",
    "neovim",
];
pub const DEFAULT_NPM_VERSION: &str = "latest";
pub const DEFAULT_WOL_MODE: &str = "magic";
pub const DEFAULT_WOL_BACKEND: &str = "network_manager";
pub const DEFAULT_WOL_REFERENCE_HOST: &str = "";
pub const DEFAULT_HOST_NAME: &str = "unknown";
pub const DEFAULT_NIS_ROLE: &str = "slave";
pub const DEFAULT_NIS_DOMAIN: &str = "";
pub const DEFAULT_NIS_MASTER: &str = "";
pub const DEFAULT_NIS_SERVER: &str = "";
pub const DEFAULT_NIS_LOCAL_ADMIN_GROUPS: &[&str] = &[];
pub const DEFAULT_SUDO_NOPASS_GROUP: &str = "superuser";
pub const DEFAULT_SUDO_NOPASS_NIS_MANAGED: bool = false;

#[derive(Debug, Clone)]
pub struct DebkitConfig {
    pub host: HostConfig,
    pub wallpapers: WallpapersConfig,
    pub variety: VarietyConfig,
    pub foundation: FoundationConfig,
    pub essentials: EssentialsConfig,
    pub npm: NpmConfig,
    pub sudo_nopass: SudoNopassConfig,
    pub nis: NisConfig,
    pub wake_on_lan: WakeOnLanConfig,
}

impl Default for DebkitConfig {
    fn default() -> Self {
        Self {
            host: HostConfig::default(),
            wallpapers: WallpapersConfig::default(),
            variety: VarietyConfig::default(),
            foundation: FoundationConfig::default(),
            essentials: EssentialsConfig::default(),
            npm: NpmConfig::default(),
            sudo_nopass: SudoNopassConfig::default(),
            nis: NisConfig::default(),
            wake_on_lan: WakeOnLanConfig::default(),
        }
    }
}

impl DebkitConfig {
    fn for_hostname(hostname: &str) -> Self {
        let mut config = Self::default();
        config.host.name = hostname.to_string();
        config.wake_on_lan.reference_host = hostname.to_string();
        config
    }
}

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub name: String,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            name: DEFAULT_HOST_NAME.to_string(),
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

#[derive(Debug, Clone)]
pub struct FoundationConfig {
    pub install: Vec<String>,
}

impl Default for FoundationConfig {
    fn default() -> Self {
        Self {
            install: DEFAULT_FOUNDATION_INSTALL
                .iter()
                .map(|target| (*target).to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EssentialsConfig {
    pub packages: Vec<String>,
}

impl Default for EssentialsConfig {
    fn default() -> Self {
        Self {
            packages: DEFAULT_ESSENTIAL_PACKAGES
                .iter()
                .map(|package| (*package).to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NpmConfig {
    pub version: String,
}

impl Default for NpmConfig {
    fn default() -> Self {
        Self {
            version: DEFAULT_NPM_VERSION.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SudoNopassConfig {
    pub enabled: bool,
    pub group: String,
    pub add_current_user: bool,
    pub users: Vec<String>,
    pub nis_managed: bool,
}

impl Default for SudoNopassConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            group: DEFAULT_SUDO_NOPASS_GROUP.to_string(),
            add_current_user: true,
            users: Vec::new(),
            nis_managed: DEFAULT_SUDO_NOPASS_NIS_MANAGED,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NisConfig {
    pub enabled: bool,
    pub role: String,
    pub domain: String,
    pub admin_user: String,
    pub local_admin_groups: Vec<String>,
    pub master: String,
    pub server: String,
    pub prefer_local: bool,
    pub push_to_slaves: bool,
    pub force_refresh_maps: bool,
    pub slaves: Vec<String>,
    pub servers: Vec<String>,
}

impl Default for NisConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            role: DEFAULT_NIS_ROLE.to_string(),
            domain: DEFAULT_NIS_DOMAIN.to_string(),
            admin_user: current_user().unwrap_or_default(),
            local_admin_groups: DEFAULT_NIS_LOCAL_ADMIN_GROUPS
                .iter()
                .map(|group| (*group).to_string())
                .collect(),
            master: DEFAULT_NIS_MASTER.to_string(),
            server: DEFAULT_NIS_SERVER.to_string(),
            prefer_local: true,
            push_to_slaves: false,
            force_refresh_maps: false,
            slaves: Vec::new(),
            servers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WakeOnLanConfig {
    pub enabled: bool,
    pub interfaces_auto: bool,
    pub interfaces: Vec<String>,
    pub mode: String,
    pub backend: String,
    pub reference_host: String,
}

impl Default for WakeOnLanConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interfaces_auto: true,
            interfaces: Vec::new(),
            mode: DEFAULT_WOL_MODE.to_string(),
            backend: DEFAULT_WOL_BACKEND.to_string(),
            reference_host: DEFAULT_WOL_REFERENCE_HOST.to_string(),
        }
    }
}

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
    let path = config_path_for_home(home);
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let (mut config, missing_keys) = parse_config(&raw)?;
    if missing_keys.host_name {
        config.host.name = hostname.to_string();
    }
    if missing_keys.wake_on_lan_reference_host {
        config.wake_on_lan.reference_host = hostname.to_string();
    }

    let host_path = host_config_path_for_home(home, hostname);
    if host_path.exists() {
        let host_raw = fs::read_to_string(&host_path)
            .with_context(|| format!("failed to read {}", host_path.display()))?;
        let (host_config, host_missing_keys) = parse_config(&host_raw)?;
        apply_host_overlay(&mut config, host_config, host_missing_keys);
    }
    config.host.name = hostname.to_string();
    validate_config(&config)?;
    Ok(config)
}

pub fn configure_complete_for_home(home: &Path) -> anyhow::Result<PathBuf> {
    let hostname = current_hostname().unwrap_or_else(|_| DEFAULT_HOST_NAME.to_string());
    let base_path = config_path_for_home(home);
    if let Some(parent) = base_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if !base_path.exists() {
        let default_cfg = DebkitConfig::for_hostname(&hostname);
        fs::write(&base_path, serialize_config(&default_cfg))
            .with_context(|| format!("failed to write {}", base_path.display()))?;
    }

    let path = host_config_path_for_home(home, &hostname);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if !path.exists() {
        let content = format!(
            "# DebKit host overrides for {hostname}\n# This file supplements ~/.config/debkit/config.toml.\n# Add only values that differ for this host.\n\n"
        );
        fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(path)
}

pub fn load_or_init_for_home(home: &Path) -> anyhow::Result<DebkitConfig> {
    let hostname = current_hostname().unwrap_or_else(|_| DEFAULT_HOST_NAME.to_string());
    let path = config_path_for_home(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if !path.exists() {
        let default_cfg = DebkitConfig::for_hostname(&hostname);
        fs::write(&path, serialize_config(&default_cfg))
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let (mut config, missing_keys) = parse_config(&raw)?;

    if missing_keys.host_name {
        config.host.name = hostname.clone();
    }
    if missing_keys.wake_on_lan_reference_host {
        config.wake_on_lan.reference_host = hostname.clone();
    }

    if missing_keys.any_missing() {
        fs::write(&path, serialize_config(&config))
            .with_context(|| format!("failed to update {}", path.display()))?;
    }

    config.host.name = hostname.clone();
    let host_path = host_config_path_for_home(home, &hostname);
    if host_path.exists() {
        let host_raw = fs::read_to_string(&host_path)
            .with_context(|| format!("failed to read {}", host_path.display()))?;
        let (host_config, host_missing_keys) = parse_config(&host_raw)?;
        apply_host_overlay(&mut config, host_config, host_missing_keys);
        config.host.name = hostname;
    }

    validate_config(&config)?;

    Ok(config)
}

pub fn config_path_for_home(home: &Path) -> PathBuf {
    home.join(".config").join("debkit").join("config.toml")
}

pub fn host_config_path_for_home(home: &Path, hostname: &str) -> PathBuf {
    home.join(".config")
        .join("debkit")
        .join("hosts")
        .join(format!("{}.toml", sanitize_hostname_for_path(hostname)))
}

pub fn home_dir() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}

fn apply_host_overlay(base: &mut DebkitConfig, overlay: DebkitConfig, missing: MissingKeys) {
    if !missing.wallpapers_folder {
        base.wallpapers.folder = overlay.wallpapers.folder;
    }
    if !missing.variety_interval_minutes {
        base.variety.interval_minutes = overlay.variety.interval_minutes;
    }
    if !missing.foundation_install {
        base.foundation.install = overlay.foundation.install;
    }
    if !missing.essentials_packages {
        base.essentials.packages = overlay.essentials.packages;
    }
    if !missing.npm_version {
        base.npm.version = overlay.npm.version;
    }
    if !missing.sudo_nopass_enabled {
        base.sudo_nopass.enabled = overlay.sudo_nopass.enabled;
    }
    if !missing.sudo_nopass_group {
        base.sudo_nopass.group = overlay.sudo_nopass.group;
    }
    if !missing.sudo_nopass_add_current_user {
        base.sudo_nopass.add_current_user = overlay.sudo_nopass.add_current_user;
    }
    if !missing.sudo_nopass_users {
        base.sudo_nopass.users = overlay.sudo_nopass.users;
    }
    if !missing.sudo_nopass_nis_managed {
        base.sudo_nopass.nis_managed = overlay.sudo_nopass.nis_managed;
    }
    if !missing.nis_enabled {
        base.nis.enabled = overlay.nis.enabled;
    }
    if !missing.nis_role {
        base.nis.role = overlay.nis.role;
    }
    if !missing.nis_domain {
        base.nis.domain = overlay.nis.domain;
    }
    if !missing.nis_admin_user {
        base.nis.admin_user = overlay.nis.admin_user;
    }
    if !missing.nis_local_admin_groups {
        base.nis.local_admin_groups = overlay.nis.local_admin_groups;
    }
    if !missing.nis_master {
        base.nis.master = overlay.nis.master;
    }
    if !missing.nis_server {
        base.nis.server = overlay.nis.server;
    }
    if !missing.nis_prefer_local {
        base.nis.prefer_local = overlay.nis.prefer_local;
    }
    if !missing.nis_push_to_slaves {
        base.nis.push_to_slaves = overlay.nis.push_to_slaves;
    }
    if !missing.nis_force_refresh_maps {
        base.nis.force_refresh_maps = overlay.nis.force_refresh_maps;
    }
    if !missing.nis_slaves {
        base.nis.slaves = overlay.nis.slaves;
    }
    if !missing.nis_servers {
        base.nis.servers = overlay.nis.servers;
    }
    if !missing.wake_on_lan_enabled {
        base.wake_on_lan.enabled = overlay.wake_on_lan.enabled;
    }
    if !missing.wake_on_lan_interfaces {
        base.wake_on_lan.interfaces_auto = overlay.wake_on_lan.interfaces_auto;
        base.wake_on_lan.interfaces = overlay.wake_on_lan.interfaces;
    }
    if !missing.wake_on_lan_mode {
        base.wake_on_lan.mode = overlay.wake_on_lan.mode;
    }
    if !missing.wake_on_lan_backend {
        base.wake_on_lan.backend = overlay.wake_on_lan.backend;
    }
    if !missing.wake_on_lan_reference_host {
        base.wake_on_lan.reference_host = overlay.wake_on_lan.reference_host;
    }
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
    if config.nis.enabled
        && matches!(config.nis.role.as_str(), "slave" | "slave-client")
        && config.nis.master.trim().is_empty()
    {
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
    Ok(())
}

fn current_user() -> Option<String> {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("USER").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
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
        DEFAULT_HOST_NAME.to_string()
    } else {
        sanitized
    }
}

fn add_nis_slave_to_raw_config(raw: &str, slave: &str) -> anyhow::Result<(String, bool)> {
    let mut document = parse_toml_document(raw)?;
    ensure_table(&mut document, "nis")?;

    let nis = document["nis"]
        .as_table_mut()
        .context("`nis` must be a TOML table")?;

    let mut slaves = match nis.get("slaves") {
        Some(item) => string_array_item(item, "nis.slaves")?,
        None => Vec::new(),
    };
    if slaves.iter().any(|existing| existing == slave) {
        return Ok((raw.to_string(), false));
    }

    slaves.push(slave.to_string());
    nis["slaves"] = array_item(&slaves);
    Ok((ensure_trailing_newline(document.to_string()), true))
}

fn ensure_trailing_newline(mut raw: String) -> String {
    if !raw.ends_with('\n') {
        raw.push('\n');
    }
    raw
}

#[derive(Debug, Clone, Copy)]
struct MissingKeys {
    host_name: bool,
    wallpapers_folder: bool,
    variety_interval_minutes: bool,
    foundation_install: bool,
    essentials_packages: bool,
    npm_version: bool,
    sudo_nopass_enabled: bool,
    sudo_nopass_group: bool,
    sudo_nopass_add_current_user: bool,
    sudo_nopass_users: bool,
    sudo_nopass_nis_managed: bool,
    nis_enabled: bool,
    nis_role: bool,
    nis_domain: bool,
    nis_admin_user: bool,
    nis_local_admin_groups: bool,
    nis_master: bool,
    nis_server: bool,
    nis_prefer_local: bool,
    nis_push_to_slaves: bool,
    nis_force_refresh_maps: bool,
    nis_slaves: bool,
    nis_servers: bool,
    wake_on_lan_enabled: bool,
    wake_on_lan_interfaces: bool,
    wake_on_lan_mode: bool,
    wake_on_lan_backend: bool,
    wake_on_lan_reference_host: bool,
}

impl MissingKeys {
    fn any_missing(self) -> bool {
        self.host_name
            || self.wallpapers_folder
            || self.variety_interval_minutes
            || self.foundation_install
            || self.essentials_packages
            || self.npm_version
            || self.sudo_nopass_enabled
            || self.sudo_nopass_group
            || self.sudo_nopass_add_current_user
            || self.sudo_nopass_users
            || self.sudo_nopass_nis_managed
            || self.nis_enabled
            || self.nis_role
            || self.nis_domain
            || self.nis_admin_user
            || self.nis_local_admin_groups
            || self.nis_master
            || self.nis_server
            || self.nis_prefer_local
            || self.nis_push_to_slaves
            || self.nis_force_refresh_maps
            || self.nis_slaves
            || self.nis_servers
            || self.wake_on_lan_enabled
            || self.wake_on_lan_interfaces
            || self.wake_on_lan_mode
            || self.wake_on_lan_backend
            || self.wake_on_lan_reference_host
    }
}

fn parse_config(raw: &str) -> anyhow::Result<(DebkitConfig, MissingKeys)> {
    let mut config = DebkitConfig::default();
    let document = parse_toml_document(raw)?;

    let host = table(&document, "host")?;
    if let Some(item) = item(host, "name") {
        config.host.name = string_item(item, "host.name")?;
    }

    let wallpapers = table(&document, "wallpapers")?;
    if let Some(item) = item(wallpapers, "folder") {
        config.wallpapers.folder = string_item(item, "wallpapers.folder")?;
    }

    let variety = table(&document, "variety")?;
    if let Some(item) = item(variety, "interval_minutes") {
        config.variety.interval_minutes = integer_item(item, "variety.interval_minutes")?;
    }

    let foundation = table(&document, "foundation")?;
    if let Some(item) = item(foundation, "install") {
        config.foundation.install = string_array_item(item, "foundation.install")?;
    }

    let essentials = table(&document, "essentials")?;
    if let Some(item) = item(essentials, "packages") {
        config.essentials.packages = string_array_item(item, "essentials.packages")?;
    }

    let npm = table(&document, "npm")?;
    if let Some(item) = item(npm, "version") {
        config.npm.version = string_item(item, "npm.version")?;
    }

    let sudo_nopass = table(&document, "sudo_nopass")?;
    if let Some(item) = item(sudo_nopass, "enabled") {
        config.sudo_nopass.enabled = bool_item(item, "sudo_nopass.enabled")?;
    }
    if let Some(item) = item(sudo_nopass, "group") {
        config.sudo_nopass.group = string_item(item, "sudo_nopass.group")?;
    }
    if let Some(item) = item(sudo_nopass, "add_current_user") {
        config.sudo_nopass.add_current_user = bool_item(item, "sudo_nopass.add_current_user")?;
    }
    if let Some(item) = item(sudo_nopass, "users") {
        config.sudo_nopass.users = string_array_item(item, "sudo_nopass.users")?;
    }
    if let Some(item) = item(sudo_nopass, "nis_managed") {
        config.sudo_nopass.nis_managed = bool_item(item, "sudo_nopass.nis_managed")?;
    }

    let nis = table(&document, "nis")?;
    if let Some(item) = item(nis, "enabled") {
        config.nis.enabled = bool_item(item, "nis.enabled")?;
    }
    if let Some(item) = item(nis, "role") {
        config.nis.role = string_item(item, "nis.role")?;
    }
    if let Some(item) = item(nis, "domain") {
        config.nis.domain = string_item(item, "nis.domain")?;
    }
    if let Some(item) = item(nis, "admin_user") {
        config.nis.admin_user = string_item(item, "nis.admin_user")?;
    }
    if let Some(item) = item(nis, "local_admin_groups") {
        config.nis.local_admin_groups = string_array_item(item, "nis.local_admin_groups")?;
    }
    if let Some(item) = item(nis, "master") {
        config.nis.master = string_item(item, "nis.master")?;
    }
    if let Some(item) = item(nis, "server") {
        config.nis.server = string_item(item, "nis.server")?;
    }
    if let Some(item) = item(nis, "prefer_local") {
        config.nis.prefer_local = bool_item(item, "nis.prefer_local")?;
    }
    if let Some(item) = item(nis, "push_to_slaves") {
        config.nis.push_to_slaves = bool_item(item, "nis.push_to_slaves")?;
    }
    if let Some(item) = item(nis, "force_refresh_maps") {
        config.nis.force_refresh_maps = bool_item(item, "nis.force_refresh_maps")?;
    }
    if let Some(item) = item(nis, "slaves") {
        config.nis.slaves = string_array_item(item, "nis.slaves")?;
    }
    if let Some(item) = item(nis, "servers") {
        config.nis.servers = string_array_item(item, "nis.servers")?;
    }

    let wake_on_lan = table(&document, "wake_on_lan")?;
    if let Some(item) = item(wake_on_lan, "enabled") {
        config.wake_on_lan.enabled = bool_item(item, "wake_on_lan.enabled")?;
    }
    if let Some(item) = item(wake_on_lan, "interfaces") {
        if item.as_str() == Some("auto") {
            config.wake_on_lan.interfaces_auto = true;
            config.wake_on_lan.interfaces.clear();
        } else {
            config.wake_on_lan.interfaces = string_array_item(item, "wake_on_lan.interfaces")?;
            config.wake_on_lan.interfaces_auto = false;
        }
    }
    if let Some(item) = item(wake_on_lan, "mode") {
        config.wake_on_lan.mode = string_item(item, "wake_on_lan.mode")?;
    }
    if let Some(item) = item(wake_on_lan, "backend").or_else(|| item(wake_on_lan, "persistence")) {
        config.wake_on_lan.backend = string_item(item, "wake_on_lan.backend")?;
    }
    if let Some(item) = item(wake_on_lan, "reference_host") {
        config.wake_on_lan.reference_host = string_item(item, "wake_on_lan.reference_host")?;
    }

    let missing = MissingKeys {
        host_name: item(host, "name").is_none(),
        wallpapers_folder: item(wallpapers, "folder").is_none(),
        variety_interval_minutes: item(variety, "interval_minutes").is_none(),
        foundation_install: item(foundation, "install").is_none(),
        essentials_packages: item(essentials, "packages").is_none(),
        npm_version: item(npm, "version").is_none(),
        sudo_nopass_enabled: item(sudo_nopass, "enabled").is_none(),
        sudo_nopass_group: item(sudo_nopass, "group").is_none(),
        sudo_nopass_add_current_user: item(sudo_nopass, "add_current_user").is_none(),
        sudo_nopass_users: item(sudo_nopass, "users").is_none(),
        sudo_nopass_nis_managed: item(sudo_nopass, "nis_managed").is_none(),
        nis_enabled: item(nis, "enabled").is_none(),
        nis_role: item(nis, "role").is_none(),
        nis_domain: item(nis, "domain").is_none(),
        nis_admin_user: item(nis, "admin_user").is_none(),
        nis_local_admin_groups: item(nis, "local_admin_groups").is_none(),
        nis_master: item(nis, "master").is_none(),
        nis_server: item(nis, "server").is_none(),
        nis_prefer_local: item(nis, "prefer_local").is_none(),
        nis_push_to_slaves: item(nis, "push_to_slaves").is_none(),
        nis_force_refresh_maps: item(nis, "force_refresh_maps").is_none(),
        nis_slaves: item(nis, "slaves").is_none(),
        nis_servers: item(nis, "servers").is_none(),
        wake_on_lan_enabled: item(wake_on_lan, "enabled").is_none(),
        wake_on_lan_interfaces: item(wake_on_lan, "interfaces").is_none(),
        wake_on_lan_mode: item(wake_on_lan, "mode").is_none(),
        wake_on_lan_backend: item(wake_on_lan, "backend")
            .or_else(|| item(wake_on_lan, "persistence"))
            .is_none(),
        wake_on_lan_reference_host: item(wake_on_lan, "reference_host").is_none(),
    };

    Ok((config, missing))
}

fn parse_toml_document(raw: &str) -> anyhow::Result<DocumentMut> {
    raw.parse::<DocumentMut>().context("invalid TOML config")
}

fn table<'a>(document: &'a DocumentMut, section: &str) -> anyhow::Result<Option<&'a Table>> {
    match document.get(section) {
        Some(item) => item
            .as_table()
            .map(Some)
            .with_context(|| format!("`{section}` must be a TOML table")),
        None => Ok(None),
    }
}

fn item<'a>(table: Option<&'a Table>, key: &str) -> Option<&'a Item> {
    table.and_then(|table| table.get(key))
}

fn string_item(item: &Item, key: &str) -> anyhow::Result<String> {
    item.as_str()
        .map(ToString::to_string)
        .with_context(|| format!("`{key}` must be a string"))
}

fn bool_item(item: &Item, key: &str) -> anyhow::Result<bool> {
    item.as_bool()
        .with_context(|| format!("`{key}` must be a boolean"))
}

fn integer_item(item: &Item, key: &str) -> anyhow::Result<u32> {
    let value = item
        .as_integer()
        .with_context(|| format!("`{key}` must be an integer"))?;
    u32::try_from(value).with_context(|| format!("`{key}` must be a non-negative u32"))
}

fn string_array_item(item: &Item, key: &str) -> anyhow::Result<Vec<String>> {
    let array = item
        .as_array()
        .with_context(|| format!("`{key}` must be an array of strings"))?;
    let mut values = Vec::with_capacity(array.len());
    for value in array {
        values.push(
            value
                .as_str()
                .map(ToString::to_string)
                .with_context(|| format!("`{key}` must be an array of strings"))?,
        );
    }
    Ok(values)
}

fn ensure_table(document: &mut DocumentMut, section: &str) -> anyhow::Result<()> {
    if document.get(section).is_none() {
        document[section] = Item::Table(Table::new());
    }
    if document[section].as_table().is_none() {
        bail!("`{section}` must be a TOML table");
    }
    Ok(())
}

fn serialize_config(config: &DebkitConfig) -> String {
    let mut document = DocumentMut::new();

    set_config_item(
        &mut document,
        "wallpapers",
        "folder",
        value(&config.wallpapers.folder),
    );
    set_config_item(
        &mut document,
        "variety",
        "interval_minutes",
        value(config.variety.interval_minutes as i64),
    );
    set_config_item(
        &mut document,
        "foundation",
        "install",
        array_item(&config.foundation.install),
    );
    set_config_item(
        &mut document,
        "essentials",
        "packages",
        array_item(&config.essentials.packages),
    );
    set_config_item(&mut document, "npm", "version", value(&config.npm.version));

    set_config_item(
        &mut document,
        "sudo_nopass",
        "enabled",
        value(config.sudo_nopass.enabled),
    );
    set_config_item(
        &mut document,
        "sudo_nopass",
        "group",
        value(&config.sudo_nopass.group),
    );
    set_config_item(
        &mut document,
        "sudo_nopass",
        "add_current_user",
        value(config.sudo_nopass.add_current_user),
    );
    set_config_item(
        &mut document,
        "sudo_nopass",
        "users",
        array_item(&config.sudo_nopass.users),
    );
    set_config_item(
        &mut document,
        "sudo_nopass",
        "nis_managed",
        value(config.sudo_nopass.nis_managed),
    );

    set_config_item(&mut document, "nis", "enabled", value(config.nis.enabled));
    set_config_item(&mut document, "nis", "role", value(&config.nis.role));
    set_config_item(&mut document, "nis", "domain", value(&config.nis.domain));
    set_config_item(
        &mut document,
        "nis",
        "admin_user",
        value(&config.nis.admin_user),
    );
    set_config_item(
        &mut document,
        "nis",
        "local_admin_groups",
        array_item(&config.nis.local_admin_groups),
    );
    set_config_item(&mut document, "nis", "master", value(&config.nis.master));
    set_config_item(&mut document, "nis", "server", value(&config.nis.server));
    set_config_item(
        &mut document,
        "nis",
        "prefer_local",
        value(config.nis.prefer_local),
    );
    set_config_item(
        &mut document,
        "nis",
        "push_to_slaves",
        value(config.nis.push_to_slaves),
    );
    set_config_item(
        &mut document,
        "nis",
        "force_refresh_maps",
        value(config.nis.force_refresh_maps),
    );
    set_config_item(
        &mut document,
        "nis",
        "slaves",
        array_item(&config.nis.slaves),
    );
    set_config_item(
        &mut document,
        "nis",
        "servers",
        array_item(&config.nis.servers),
    );

    set_config_item(
        &mut document,
        "wake_on_lan",
        "enabled",
        value(config.wake_on_lan.enabled),
    );
    if config.wake_on_lan.interfaces_auto {
        set_config_item(&mut document, "wake_on_lan", "interfaces", value("auto"));
    } else {
        set_config_item(
            &mut document,
            "wake_on_lan",
            "interfaces",
            array_item(&config.wake_on_lan.interfaces),
        );
    }
    set_config_item(
        &mut document,
        "wake_on_lan",
        "mode",
        value(&config.wake_on_lan.mode),
    );
    set_config_item(
        &mut document,
        "wake_on_lan",
        "backend",
        value(&config.wake_on_lan.backend),
    );
    set_config_item(
        &mut document,
        "wake_on_lan",
        "reference_host",
        value(&config.wake_on_lan.reference_host),
    );

    ensure_trailing_newline(document.to_string())
}

fn set_config_item(document: &mut DocumentMut, section: &str, key: &str, item: Item) {
    if document.get(section).is_none() {
        document[section] = Item::Table(Table::new());
    }
    document[section]
        .as_table_mut()
        .expect("section was just created as table")[key] = item;
}

fn array_item(items: &[String]) -> Item {
    let mut array = Array::default();
    for item in items {
        array.push(item.as_str());
    }
    Item::Value(Value::Array(array))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn initializes_default_config() {
        let home = temp_home("default_init");
        let config = load_or_init_for_home(&home).unwrap();

        assert_ne!(config.host.name, DEFAULT_HOST_NAME);
        assert_eq!(config.wallpapers.folder, DEFAULT_WALLPAPERS_FOLDER);
        assert_eq!(config.variety.interval_minutes, DEFAULT_INTERVAL_MINUTES);
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
            DEFAULT_ESSENTIAL_PACKAGES.to_vec()
        );
        assert_eq!(config.npm.version, DEFAULT_NPM_VERSION);
        assert!(!config.sudo_nopass.enabled);
        assert!(!config.nis.enabled);
        assert_eq!(config.nis.role, DEFAULT_NIS_ROLE);
        assert_eq!(config.nis.domain, DEFAULT_NIS_DOMAIN);
        let expected_user = std::env::var("SUDO_USER")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_default();
        assert_eq!(config.nis.admin_user, expected_user.trim());
        assert_eq!(
            config.nis.local_admin_groups,
            DEFAULT_NIS_LOCAL_ADMIN_GROUPS.to_vec()
        );
        assert_eq!(config.nis.master, DEFAULT_NIS_MASTER);
        assert_eq!(config.nis.server, DEFAULT_NIS_SERVER);
        assert!(config.nis.prefer_local);
        assert!(!config.nis.push_to_slaves);
        assert!(!config.nis.force_refresh_maps);
        assert!(config.nis.slaves.is_empty());
        assert!(config.nis.servers.is_empty());
        assert!(config.wake_on_lan.enabled);
        assert_eq!(config.wake_on_lan.mode, DEFAULT_WOL_MODE);
        assert_eq!(config.wake_on_lan.backend, DEFAULT_WOL_BACKEND);
        assert_eq!(config.wake_on_lan.reference_host, config.host.name);
        assert!(config.wake_on_lan.interfaces_auto);

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
            DEFAULT_ESSENTIAL_PACKAGES.to_vec()
        );
        assert_eq!(config.npm.version, DEFAULT_NPM_VERSION);
        assert!(!config.sudo_nopass.enabled);
        assert_eq!(config.sudo_nopass.group, DEFAULT_SUDO_NOPASS_GROUP);
        assert!(config.sudo_nopass.add_current_user);
        assert!(config.sudo_nopass.users.is_empty());
        assert!(!config.nis.enabled);

        let rewritten = fs::read_to_string(config_path).unwrap();
        assert!(rewritten.contains("interval_minutes"));
        assert!(rewritten.contains("/tmp/walls"));
        assert!(rewritten.contains(
            "install = [\"essentials\", \"git\", \"ripgrep\", \"rust\", \"npm\", \"codex\", \"variety\", \"nis\", \"wake-on-lan\"]"
        ));
        assert!(rewritten.contains("[essentials]"));
        assert!(rewritten.contains("packages = [\"curl\", \"wget\", \"zip\", \"unzip\", \"rsync\", \"ca-certificates\", \"gnupg\", \"apt-transport-https\", \"neovim\"]"));
        assert!(rewritten.contains("[sudo_nopass]"));
        assert!(rewritten.contains("enabled = false"));
        assert!(rewritten.contains("version = \"latest\""));
        assert!(rewritten.contains("[nis]"));
        assert!(rewritten.contains("enabled = false"));
        assert!(rewritten.contains("[wake_on_lan]"));
        assert!(rewritten.contains(&format!("reference_host = \"{}\"", config.host.name)));
    }

    #[test]
    fn parses_foundation_install_array() {
        let raw = "[foundation]\ninstall = [\"variety\", \"rust\"]\n";
        let (config, missing) = parse_config(raw).unwrap();
        assert_eq!(config.foundation.install, vec!["variety", "rust"]);
        assert!(missing.wallpapers_folder);
        assert!(missing.variety_interval_minutes);
        assert!(!missing.foundation_install);
        assert!(missing.npm_version);
    }

    #[test]
    fn parses_essentials_packages_array() {
        let raw = "[essentials]\npackages = [\"curl\", \"jq\"]\n";
        let (config, missing) = parse_config(raw).unwrap();
        assert_eq!(config.essentials.packages, vec!["curl", "jq"]);
        assert!(!missing.essentials_packages);
    }

    #[test]
    fn parses_multiline_arrays() {
        let raw = "[foundation]\ninstall = [\n    \"git\",\n    \"ripgrep\",\n]\n\n[nis]\nlocal_admin_groups = [\n    \"superuser\",\n]\nslaves = [\n    \"node-a.example.lan\",\n    \"node-b.example.lan\",\n]\n";
        let (config, missing) = parse_config(raw).unwrap();
        assert_eq!(config.foundation.install, vec!["git", "ripgrep"]);
        assert_eq!(config.nis.local_admin_groups, vec!["superuser"]);
        assert_eq!(
            config.nis.slaves,
            vec!["node-a.example.lan", "node-b.example.lan"]
        );
        assert!(!missing.foundation_install);
        assert!(!missing.nis_local_admin_groups);
        assert!(!missing.nis_slaves);
    }

    #[test]
    fn configures_complete_host_config() {
        let home = temp_home("complete_host");
        let path = configure_complete_for_home(&home).unwrap();
        let hostname = current_hostname().unwrap();
        assert_eq!(path, host_config_path_for_home(&home, &hostname));
        let raw = fs::read_to_string(path).unwrap();
        assert!(raw.contains(&format!("DebKit host overrides for {hostname}")));
        assert!(raw.contains("supplements ~/.config/debkit/config.toml"));

        let base_raw = fs::read_to_string(config_path_for_home(&home)).unwrap();
        assert!(base_raw.contains("[wake_on_lan]"));
    }

    #[test]
    fn host_config_overlay_supplements_base_config() {
        let home = temp_home("host_overlay");
        let hostname = current_hostname().unwrap();
        let base_path = config_path_for_home(&home);
        fs::create_dir_all(base_path.parent().unwrap()).unwrap();
        fs::write(
            &base_path,
            "[foundation]\ninstall = [\"git\"]\n[wake_on_lan]\nbackend = \"network_manager\"\n",
        )
        .unwrap();

        let path = host_config_path_for_home(&home, &hostname);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[wake_on_lan]\nenabled = true\n").unwrap();

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
            PathBuf::from("/tmp/home/.config/debkit/hosts/bad_name.toml")
        );
    }

    #[test]
    fn parses_npm_version() {
        let raw = "[npm]\nversion = \"24.12.0\"\n";
        let (config, missing) = parse_config(raw).unwrap();
        assert_eq!(config.npm.version, "24.12.0");
        assert!(missing.wallpapers_folder);
        assert!(missing.variety_interval_minutes);
        assert!(missing.foundation_install);
        assert!(!missing.npm_version);
    }

    #[test]
    fn parses_sudo_nopass_config() {
        let raw = "[sudo_nopass]\nenabled = true\ngroup = \"superuser\"\nadd_current_user = false\nusers = [\"alice\", \"bob\"]\n";
        let (config, missing) = parse_config(raw).unwrap();
        assert!(config.sudo_nopass.enabled);
        assert_eq!(config.sudo_nopass.group, "superuser");
        assert!(!config.sudo_nopass.add_current_user);
        assert_eq!(config.sudo_nopass.users, vec!["alice", "bob"]);
        assert!(!missing.sudo_nopass_enabled);
        assert!(!missing.sudo_nopass_group);
        assert!(!missing.sudo_nopass_add_current_user);
        assert!(!missing.sudo_nopass_users);
    }

    #[test]
    fn parses_nis_config() {
        let raw = "[nis]\nenabled = true\nrole = \"master\"\ndomain = \"example.internal\"\nadmin_user = \"admin\"\nlocal_admin_groups = [\"sudo\", \"wheel\"]\nmaster = \"iris.example.internal\"\nserver = \"iris.example.internal\"\nprefer_local = false\npush_to_slaves = true\nforce_refresh_maps = true\nslaves = [\"spitfire.example.internal\", \"laptop.example.internal\"]\nservers = [\"legacy1\", \"legacy2\"]\n";
        let (config, missing) = parse_config(raw).unwrap();
        assert!(config.nis.enabled);
        assert_eq!(config.nis.role, "master");
        assert_eq!(config.nis.domain, "example.internal");
        assert_eq!(config.nis.admin_user, "admin");
        assert_eq!(config.nis.local_admin_groups, vec!["sudo", "wheel"]);
        assert_eq!(config.nis.master, "iris.example.internal");
        assert_eq!(config.nis.server, "iris.example.internal");
        assert!(!config.nis.prefer_local);
        assert!(config.nis.push_to_slaves);
        assert!(config.nis.force_refresh_maps);
        assert_eq!(
            config.nis.slaves,
            vec!["spitfire.example.internal", "laptop.example.internal"]
        );
        assert_eq!(config.nis.servers, vec!["legacy1", "legacy2"]);
        assert!(!missing.nis_enabled);
        assert!(!missing.nis_role);
        assert!(!missing.nis_domain);
        assert!(!missing.nis_admin_user);
        assert!(!missing.nis_local_admin_groups);
        assert!(!missing.nis_master);
        assert!(!missing.nis_server);
        assert!(!missing.nis_prefer_local);
        assert!(!missing.nis_push_to_slaves);
        assert!(!missing.nis_force_refresh_maps);
        assert!(!missing.nis_slaves);
        assert!(!missing.nis_servers);
    }

    #[test]
    fn parses_wake_on_lan_config() {
        let raw = "[wake_on_lan]\nenabled = true\ninterfaces = [\"enp5s0\"]\nmode = \"magic\"\nbackend = \"ethtool\"\nreference_host = \"workstation\"\n";
        let (config, missing) = parse_config(raw).unwrap();
        assert!(config.wake_on_lan.enabled);
        assert!(!config.wake_on_lan.interfaces_auto);
        assert_eq!(config.wake_on_lan.interfaces, vec!["enp5s0"]);
        assert_eq!(config.wake_on_lan.mode, "magic");
        assert_eq!(config.wake_on_lan.backend, "ethtool");
        assert_eq!(config.wake_on_lan.reference_host, "workstation");
        assert!(!missing.wake_on_lan_enabled);
        assert!(!missing.wake_on_lan_interfaces);
    }

    #[test]
    fn parses_wake_on_lan_auto_interfaces() {
        let raw = "[wake_on_lan]\ninterfaces = \"auto\"\nbackend = \"network_manager\"\n";
        let (config, _) = parse_config(raw).unwrap();
        assert!(config.wake_on_lan.interfaces_auto);
        assert!(config.wake_on_lan.interfaces.is_empty());
        assert_eq!(config.wake_on_lan.backend, "network_manager");
    }

    #[test]
    fn add_nis_slave_updates_existing_slaves_array() {
        let raw = "[nis]\nenabled = true\nrole = \"master\"\nslaves = [\"node-a.example.lan\"]\n";
        let (updated, added) = add_nis_slave_to_raw_config(raw, "node-b.example.lan").unwrap();
        assert!(added);
        assert!(updated.contains("slaves = [\"node-a.example.lan\", \"node-b.example.lan\"]"));
    }

    #[test]
    fn add_nis_slave_is_idempotent() {
        let raw = "[nis]\nslaves = [\"node-a.example.lan\"]\n";
        let (updated, added) = add_nis_slave_to_raw_config(raw, "node-a.example.lan").unwrap();
        assert!(!added);
        assert_eq!(updated, raw);
    }

    #[test]
    fn add_nis_slave_inserts_missing_slaves_key() {
        let raw = "[nis]\nenabled = true\nrole = \"master\"\n\n[wake_on_lan]\nenabled = true\n";
        let (updated, added) = add_nis_slave_to_raw_config(raw, "node-a.example.lan").unwrap();
        assert!(added);
        let (config, missing) = parse_config(&updated).unwrap();
        assert_eq!(config.nis.slaves, vec!["node-a.example.lan"]);
        assert!(!missing.nis_slaves);
        assert!(config.wake_on_lan.enabled);
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
