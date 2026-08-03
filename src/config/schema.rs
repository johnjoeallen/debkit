use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
pub const DEFAULT_HOST_NAME: &str = "unknown";
pub const DEFAULT_NIS_ROLE: &str = "slave";
pub const DEFAULT_SUDO_NOPASS_GROUP: &str = "superuser";
pub const DEFAULT_GIT_CREDENTIAL_HELPER: &str = "store";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
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
    pub git: GitConfig,
    pub apt: AptConfig,
    pub pam: PamConfig,
}

impl DebkitConfig {
    pub fn for_hostname(hostname: &str) -> Self {
        let mut config = Self::default();
        config.host.name = hostname.to_string();
        config.wake_on_lan.reference_host = hostname.to_string();
        config
    }
}

/// The on-disk shape: everything lives under a top-level `debkit:` key, matching every
/// example in the v2 requirements doc.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebkitConfigFile {
    #[serde(default)]
    pub debkit: DebkitConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    #[serde(default = "default_host_name")]
    pub name: String,
}

fn default_host_name() -> String {
    DEFAULT_HOST_NAME.to_string()
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            name: default_host_name(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VarietyConfig {
    #[serde(default = "default_interval_minutes")]
    pub interval_minutes: u32,
}

fn default_interval_minutes() -> u32 {
    DEFAULT_INTERVAL_MINUTES
}

impl Default for VarietyConfig {
    fn default() -> Self {
        Self {
            interval_minutes: default_interval_minutes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FoundationConfig {
    #[serde(default = "default_foundation_install")]
    pub install: Vec<String>,
}

fn default_foundation_install() -> Vec<String> {
    DEFAULT_FOUNDATION_INSTALL
        .iter()
        .map(|target| (*target).to_string())
        .collect()
}

impl Default for FoundationConfig {
    fn default() -> Self {
        Self {
            install: default_foundation_install(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EssentialsConfig {
    #[serde(default = "default_essential_packages")]
    pub packages: Vec<String>,
}

fn default_essential_packages() -> Vec<String> {
    DEFAULT_ESSENTIAL_PACKAGES
        .iter()
        .map(|package| (*package).to_string())
        .collect()
}

impl Default for EssentialsConfig {
    fn default() -> Self {
        Self {
            packages: default_essential_packages(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NpmConfig {
    #[serde(default = "default_npm_version")]
    pub version: String,
}

fn default_npm_version() -> String {
    DEFAULT_NPM_VERSION.to_string()
}

impl Default for NpmConfig {
    fn default() -> Self {
        Self {
            version: default_npm_version(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SudoNopassConfig {
    pub enabled: bool,
    #[serde(default = "default_sudo_nopass_group")]
    pub group: String,
    #[serde(default = "default_true")]
    pub add_current_user: bool,
    pub users: Vec<String>,
    pub nis_managed: bool,
}

fn default_sudo_nopass_group() -> String {
    DEFAULT_SUDO_NOPASS_GROUP.to_string()
}

fn default_true() -> bool {
    true
}

impl Default for SudoNopassConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            group: default_sudo_nopass_group(),
            add_current_user: true,
            users: Vec::new(),
            nis_managed: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NisConfig {
    pub enabled: bool,
    #[serde(default = "default_nis_role")]
    pub role: String,
    pub domain: String,
    #[serde(default = "default_nis_admin_user")]
    pub admin_user: String,
    pub local_admin_groups: Vec<String>,
    pub master: String,
    pub server: String,
    #[serde(default = "default_true")]
    pub prefer_local: bool,
    pub push_to_slaves: bool,
    pub force_refresh_maps: bool,
    pub slaves: Vec<String>,
    pub servers: Vec<String>,
}

fn default_nis_role() -> String {
    DEFAULT_NIS_ROLE.to_string()
}

fn default_nis_admin_user() -> String {
    current_user().unwrap_or_default()
}

impl Default for NisConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            role: default_nis_role(),
            domain: String::new(),
            admin_user: default_nis_admin_user(),
            local_admin_groups: Vec::new(),
            master: String::new(),
            server: String::new(),
            prefer_local: true,
            push_to_slaves: false,
            force_refresh_maps: false,
            slaves: Vec::new(),
            servers: Vec::new(),
        }
    }
}

fn current_user() -> Option<String> {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("USER").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
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
            reference_host: String::new(),
        }
    }
}

/// `interfaces: auto` or `interfaces: [enp5s0, ...]` in YAML. Kept as a separate wire
/// type so the runtime `WakeOnLanConfig` can keep its plain `interfaces_auto: bool` +
/// `interfaces: Vec<String>` shape that `install::wake_on_lan` already consumes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum InterfacesSpec {
    Auto(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WakeOnLanConfigWire {
    enabled: bool,
    interfaces: InterfacesSpec,
    mode: String,
    backend: String,
    reference_host: String,
}

impl Default for WakeOnLanConfigWire {
    fn default() -> Self {
        let defaults = WakeOnLanConfig::default();
        Self {
            enabled: defaults.enabled,
            interfaces: InterfacesSpec::Auto("auto".to_string()),
            mode: defaults.mode,
            backend: defaults.backend,
            reference_host: defaults.reference_host,
        }
    }
}

impl From<WakeOnLanConfigWire> for WakeOnLanConfig {
    fn from(wire: WakeOnLanConfigWire) -> Self {
        let (interfaces_auto, interfaces) = match wire.interfaces {
            InterfacesSpec::Auto(_) => (true, Vec::new()),
            InterfacesSpec::List(list) => (false, list),
        };
        Self {
            enabled: wire.enabled,
            interfaces_auto,
            interfaces,
            mode: wire.mode,
            backend: wire.backend,
            reference_host: wire.reference_host,
        }
    }
}

impl From<&WakeOnLanConfig> for WakeOnLanConfigWire {
    fn from(config: &WakeOnLanConfig) -> Self {
        Self {
            enabled: config.enabled,
            interfaces: if config.interfaces_auto {
                InterfacesSpec::Auto("auto".to_string())
            } else {
                InterfacesSpec::List(config.interfaces.clone())
            },
            mode: config.mode.clone(),
            backend: config.backend.clone(),
            reference_host: config.reference_host.clone(),
        }
    }
}

impl<'de> Deserialize<'de> for WakeOnLanConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        WakeOnLanConfigWire::deserialize(deserializer).map(Into::into)
    }
}

impl Serialize for WakeOnLanConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        WakeOnLanConfigWire::from(self).serialize(serializer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    pub enabled: bool,
    /// `"store"`, `"cache"`, `"none"` (no helper managed), or an explicit
    /// `credential.helper` command string.
    #[serde(default = "default_git_credential_helper")]
    pub credential_helper: String,
    /// Only consulted when `credential_helper = "store"`. Empty means git's own
    /// default (`~/.git-credentials`).
    pub credential_store_file: String,
}

fn default_git_credential_helper() -> String {
    DEFAULT_GIT_CREDENTIAL_HELPER.to_string()
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            credential_helper: default_git_credential_helper(),
            credential_store_file: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AptConfig {
    pub enabled: bool,
    /// e.g. `"http://10.0.0.1:3142"`. Empty means no proxy is declared/managed.
    pub proxy: String,
    /// Hosts that should bypass `proxy` via a per-host `DIRECT` override.
    pub direct_hosts: Vec<String>,
}

pub const DEFAULT_PAM_SKELETON: &str = "/etc/skel";
pub const DEFAULT_PAM_UMASK: &str = "0022";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PamConfig {
    pub create_home_on_first_login: bool,
    #[serde(default = "default_pam_services")]
    pub services: Vec<String>,
    #[serde(default = "default_pam_skeleton")]
    pub skeleton: String,
    #[serde(default = "default_pam_umask")]
    pub umask: String,
}

fn default_pam_services() -> Vec<String> {
    vec!["login".to_string(), "sshd".to_string()]
}

fn default_pam_skeleton() -> String {
    DEFAULT_PAM_SKELETON.to_string()
}

fn default_pam_umask() -> String {
    DEFAULT_PAM_UMASK.to_string()
}

impl Default for PamConfig {
    fn default() -> Self {
        Self {
            create_home_on_first_login: false,
            services: default_pam_services(),
            skeleton: default_pam_skeleton(),
            umask: default_pam_umask(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_wake_on_lan_auto_interfaces() {
        let yaml = "enabled: true\ninterfaces: auto\nbackend: network_manager\n";
        let config: WakeOnLanConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(config.interfaces_auto);
        assert!(config.interfaces.is_empty());
        assert_eq!(config.backend, "network_manager");
    }

    #[test]
    fn deserializes_wake_on_lan_explicit_interfaces() {
        let yaml = "interfaces: [enp5s0]\nbackend: ethtool\n";
        let config: WakeOnLanConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(!config.interfaces_auto);
        assert_eq!(config.interfaces, vec!["enp5s0"]);
    }

    #[test]
    fn round_trips_wake_on_lan_through_yaml() {
        let config = WakeOnLanConfig {
            enabled: true,
            interfaces_auto: false,
            interfaces: vec!["enp5s0".to_string()],
            mode: "magic".to_string(),
            backend: "ethtool".to_string(),
            reference_host: "spitfire".to_string(),
        };
        let rendered = serde_yaml_ng::to_string(&config).unwrap();
        let parsed: WakeOnLanConfig = serde_yaml_ng::from_str(&rendered).unwrap();
        assert_eq!(parsed.interfaces, vec!["enp5s0"]);
        assert!(!parsed.interfaces_auto);
        assert_eq!(parsed.reference_host, "spitfire");
    }

    #[test]
    fn default_config_matches_v1_defaults() {
        let config = DebkitConfig::default();
        assert_eq!(config.variety.interval_minutes, DEFAULT_INTERVAL_MINUTES);
        assert_eq!(
            config.foundation.install,
            DEFAULT_FOUNDATION_INSTALL
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(config.nis.role, DEFAULT_NIS_ROLE);
        assert!(config.wake_on_lan.enabled);
        assert!(config.wake_on_lan.interfaces_auto);
    }

    #[test]
    fn partial_yaml_backfills_missing_fields_with_defaults() {
        let file: DebkitConfigFile =
            serde_yaml_ng::from_str("debkit:\n  wallpapers:\n    folder: /tmp/walls\n").unwrap();
        assert_eq!(file.debkit.wallpapers.folder, "/tmp/walls");
        assert_eq!(
            file.debkit.variety.interval_minutes,
            DEFAULT_INTERVAL_MINUTES
        );
        assert_eq!(file.debkit.npm.version, DEFAULT_NPM_VERSION);
    }
}
