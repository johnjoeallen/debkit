use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::config::DebkitConfig;

#[derive(Debug, Clone)]
pub struct VarietyStatus {
    pub installed_version: Option<String>,
    pub wallpapers_folder: String,
    pub wallpapers_folder_exists: bool,
    pub autostart_exists: bool,
}

pub fn run(config: &DebkitConfig) -> anyhow::Result<()> {
    install_variety_package()?;

    if !command_available("variety") {
        bail!("`variety` was not found on PATH after installation");
    }

    let user = target_user_context()?;
    configure_variety(&user, config)?;

    let status = collect_status_for_user(config, &user)?;
    print_status_report(&status);

    if is_gnome_desktop() {
        println!(
            "Note: If the tray icon is missing on GNOME, AppIndicator extension may be absent. Wallpaper rotation still works without tray support."
        );
    }

    Ok(())
}

pub fn print_status(config: &DebkitConfig) -> anyhow::Result<()> {
    let user = target_user_context()?;
    let status = collect_status_for_user(config, &user)?;
    print_status_report(&status);
    Ok(())
}

fn configure_variety(user: &UserContext, config: &DebkitConfig) -> anyhow::Result<()> {
    let wallpapers_dir = Path::new(&config.wallpapers.folder);
    if !wallpapers_dir.exists() {
        eprintln!(
            "warning: wallpapers folder does not exist: {}",
            wallpapers_dir.display()
        );
    }

    let config_dir = user.home.join(".config");
    fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    ensure_owned_writable_dir(&config_dir, user)?;

    let variety_dir = config_dir.join("variety");
    fs::create_dir_all(&variety_dir)
        .with_context(|| format!("failed to create {}", variety_dir.display()))?;
    ensure_owned_writable_dir(&variety_dir, user)?;

    let conf_path = variety_dir.join("variety.conf");
    ensure_variety_conf(
        &conf_path,
        &config.wallpapers.folder,
        config.variety.interval_minutes,
    )?;
    ensure_owned_writable_file(&conf_path, user)?;

    configure_gsettings_best_effort(config);

    let autostart_path = user
        .home
        .join(".config")
        .join("autostart")
        .join("variety.desktop");
    ensure_autostart_desktop(&autostart_path)?;
    if let Some(parent) = autostart_path.parent() {
        ensure_owned_writable_dir(parent, user)?;
    }
    ensure_owned_writable_file(&autostart_path, user)?;

    Ok(())
}

fn install_variety_package() -> anyhow::Result<()> {
    run_apt_command(&["update"])?;
    run_apt_command(&["install", "-y", "variety"])?;
    Ok(())
}

fn run_apt_command(args: &[&str]) -> anyhow::Result<()> {
    let euid = current_euid()?;

    let mut command;
    if euid == 0 {
        command = Command::new("apt-get");
        command.args(args);
    } else if command_available("sudo") {
        command = Command::new("sudo");
        command.arg("apt-get").args(args);
    } else {
        bail!(
            "installing Variety requires root privileges; run as root or install `sudo` and retry"
        );
    }

    let status = command
        .env("DEBIAN_FRONTEND", "noninteractive")
        .status()
        .context("failed to launch apt-get")?;
    if !status.success() {
        bail!("apt-get {} failed with status {}", args.join(" "), status);
    }

    Ok(())
}

fn ensure_variety_conf(path: &Path, folder: &str, interval_minutes: u32) -> anyhow::Result<()> {
    let existing = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?
    } else {
        default_variety_conf().unwrap_or_default()
    };

    let updated = configure_variety_conf_text(&existing, folder, interval_minutes);
    if updated != existing {
        fs::write(path, updated).with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(())
}

fn default_variety_conf() -> Option<String> {
    fs::read_to_string("/usr/share/variety/config/variety.conf").ok()
}

fn configure_variety_conf_text(existing: &str, folder: &str, interval_minutes: u32) -> String {
    let interval_seconds = interval_minutes.saturating_mul(60).max(5);
    let mut lines = existing
        .lines()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    upsert_root_key(&mut lines, "change_enabled", "True");
    upsert_root_key(&mut lines, "change_on_start", "True");
    upsert_root_key(&mut lines, "change_interval", &interval_seconds.to_string());
    upsert_root_key(&mut lines, "internet_enabled", "False");
    upsert_root_key(&mut lines, "wallpaper_auto_rotate", "True");

    upsert_root_key(&mut lines, "smart_notice_shown", "True");
    upsert_root_key(&mut lines, "smart_register_shown", "True");
    upsert_root_key(&mut lines, "stats_notice_shown", "True");

    set_section(
        &mut lines,
        "sources",
        &[format!("src1 = True|folder|{folder}")],
    );

    to_text(lines)
}

fn upsert_root_key(lines: &mut Vec<String>, key: &str, value: &str) {
    let mut first_idx = None;
    let mut to_remove = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            continue;
        }
        let Some((line_key, _)) = parse_key_value(trimmed) else {
            continue;
        };
        if line_key != key {
            continue;
        }
        if first_idx.is_none() {
            first_idx = Some(idx);
        } else {
            to_remove.push(idx);
        }
    }

    for idx in to_remove.into_iter().rev() {
        lines.remove(idx);
    }

    if let Some(idx) = first_idx {
        lines[idx] = format!("{key} = {value}");
        return;
    }

    let insert_at = lines
        .iter()
        .position(|line| {
            let trimmed = line.trim();
            trimmed.starts_with('[') && trimmed.ends_with(']')
        })
        .unwrap_or(lines.len());
    lines.insert(insert_at, format!("{key} = {value}"));
}

fn set_section(lines: &mut Vec<String>, section: &str, section_lines: &[String]) {
    let section_header = format!("[{section}]");
    let start = lines
        .iter()
        .position(|line| line.trim() == section_header.as_str());

    if let Some(start_idx) = start {
        let end_idx = lines
            .iter()
            .enumerate()
            .skip(start_idx + 1)
            .find_map(|(idx, line)| {
                let trimmed = line.trim();
                if trimmed.starts_with('[') && trimmed.ends_with(']') {
                    Some(idx)
                } else {
                    None
                }
            })
            .unwrap_or(lines.len());

        lines.splice(start_idx + 1..end_idx, section_lines.iter().cloned());
        return;
    }

    if !lines.is_empty() && !lines.last().is_some_and(|line| line.is_empty()) {
        lines.push(String::new());
    }
    lines.push(section_header);
    lines.extend(section_lines.iter().cloned());
}

fn parse_key_value(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('#') || line.is_empty() {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

fn to_text(lines: Vec<String>) -> String {
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn configure_gsettings_best_effort(config: &DebkitConfig) {
    if !command_available("gsettings") {
        return;
    }

    let interval_seconds = config.variety.interval_minutes.saturating_mul(60);
    let folder = config.wallpapers.folder.replace('"', "\\\"");
    let folder_uri = format!("file://{folder}");

    let attempts = [
        ("org.variety", "sources", format!("['{folder_uri}']")),
        ("org.variety", "source-folders", format!("['{folder}']")),
        (
            "org.variety",
            "change-interval",
            interval_seconds.to_string(),
        ),
        ("org.variety", "download-enabled", "false".to_string()),
    ];

    for (schema, key, value) in attempts {
        let writable = Command::new("gsettings")
            .args(["writable", schema, key])
            .output();
        let Ok(output) = writable else {
            continue;
        };
        if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != "true" {
            continue;
        }

        let _ = Command::new("gsettings")
            .args(["set", schema, key, &value])
            .status();
    }
}

fn ensure_autostart_desktop(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let base = if Path::new("/usr/share/applications/variety.desktop").exists() {
        fs::read_to_string("/usr/share/applications/variety.desktop")
            .context("failed to read /usr/share/applications/variety.desktop")?
    } else {
        "[Desktop Entry]\nType=Application\nName=Variety\nExec=variety\n".to_string()
    };

    let existing = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };

    let desired = normalize_desktop_entry(if existing.is_empty() {
        &base
    } else {
        &existing
    });
    if existing != desired {
        fs::write(path, desired).with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(())
}

fn normalize_desktop_entry(content: &str) -> String {
    let mut lines = if content.contains("[Desktop Entry]") {
        content.lines().map(ToString::to_string).collect::<Vec<_>>()
    } else {
        vec!["[Desktop Entry]".to_string()]
    };

    lines = upsert_desktop_key(lines, "X-GNOME-Autostart-enabled", "true");
    lines = upsert_desktop_key(lines, "Hidden", "false");

    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn upsert_desktop_key(lines: Vec<String>, key: &str, value: &str) -> Vec<String> {
    let prefix = format!("{key}=");
    let mut out = Vec::with_capacity(lines.len() + 1);
    let mut seen = false;

    for line in lines {
        if line.starts_with(&prefix) {
            if !seen {
                out.push(format!("{key}={value}"));
                seen = true;
            }
            continue;
        }
        out.push(line);
    }

    if !seen {
        out.push(format!("{key}={value}"));
    }

    out
}

fn collect_status_for_user(
    config: &DebkitConfig,
    user: &UserContext,
) -> anyhow::Result<VarietyStatus> {
    let installed_version = installed_variety_version();
    let autostart = user
        .home
        .join(".config")
        .join("autostart")
        .join("variety.desktop");

    Ok(VarietyStatus {
        installed_version,
        wallpapers_folder: config.wallpapers.folder.clone(),
        wallpapers_folder_exists: Path::new(&config.wallpapers.folder).exists(),
        autostart_exists: autostart.exists(),
    })
}

fn installed_variety_version() -> Option<String> {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${Version}", "variety"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

fn print_status_report(status: &VarietyStatus) {
    let version = status
        .installed_version
        .as_deref()
        .unwrap_or("not installed");
    println!("Variety status:");
    println!("- installed version: {version}");
    println!("- wallpapers folder: {}", status.wallpapers_folder);
    println!(
        "- wallpapers folder exists: {}",
        status.wallpapers_folder_exists
    );
    println!("- autostart entry exists: {}", status.autostart_exists);
}

fn command_available(program: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {program} >/dev/null 2>&1")])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn current_euid() -> anyhow::Result<u32> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to get current uid")?;
    if !output.status.success() {
        bail!("failed to determine current uid");
    }

    let uid = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .context("failed to parse uid")?;
    Ok(uid)
}

#[derive(Debug, Clone)]
struct UserContext {
    home: PathBuf,
    uid: Option<u32>,
    gid: Option<u32>,
}

fn target_user_context() -> anyhow::Result<UserContext> {
    let euid = current_euid()?;
    if euid == 0 {
        if let Some(sudo_user) = env::var_os("SUDO_USER") {
            let sudo_user = sudo_user.to_string_lossy().trim().to_string();
            if !sudo_user.is_empty() {
                if let Some(entry) = passwd_entry_for_user(&sudo_user) {
                    return Ok(UserContext {
                        home: entry.home,
                        uid: Some(entry.uid),
                        gid: Some(entry.gid),
                    });
                }
                return Ok(UserContext {
                    home: PathBuf::from(format!("/home/{sudo_user}")),
                    uid: None,
                    gid: None,
                });
            }
        }
    }

    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")?;
    Ok(UserContext {
        home,
        uid: None,
        gid: None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PasswdEntry {
    uid: u32,
    gid: u32,
    home: PathBuf,
}

fn passwd_entry_for_user(user: &str) -> Option<PasswdEntry> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd_entry_for_user_from_passwd(user, &passwd)
}

fn passwd_entry_for_user_from_passwd(user: &str, passwd: &str) -> Option<PasswdEntry> {
    for line in passwd.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        let mut parts = line.split(':');
        let name = parts.next()?;
        if name != user {
            continue;
        }

        let _password = parts.next()?;
        let uid = parts.next()?.parse::<u32>().ok()?;
        let gid = parts.next()?.parse::<u32>().ok()?;
        let _gecos = parts.next()?;
        let home = PathBuf::from(parts.next()?);
        return Some(PasswdEntry { uid, gid, home });
    }

    None
}

fn ensure_owned_writable_dir(path: &Path, user: &UserContext) -> anyhow::Result<()> {
    set_mode(path, 0o755)?;
    if let (Some(uid), Some(gid)) = (user.uid, user.gid) {
        chown_path(path, uid, gid)?;
    }
    Ok(())
}

fn ensure_owned_writable_file(path: &Path, user: &UserContext) -> anyhow::Result<()> {
    set_mode(path, 0o644)?;
    if let (Some(uid), Some(gid)) = (user.uid, user.gid) {
        chown_path(path, uid, gid)?;
    }
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> anyhow::Result<()> {
    let mut perms = fs::metadata(path)
        .with_context(|| format!("failed to read metadata for {}", path.display()))?
        .permissions();
    perms.set_mode(mode);
    fs::set_permissions(path, perms)
        .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    Ok(())
}

fn chown_path(path: &Path, uid: u32, gid: u32) -> anyhow::Result<()> {
    let status = Command::new("chown")
        .arg(format!("{uid}:{gid}"))
        .arg(path)
        .status()
        .with_context(|| format!("failed to start chown for {}", path.display()))?;
    if !status.success() {
        bail!(
            "failed to set ownership on {} to {uid}:{gid}",
            path.display()
        );
    }
    Ok(())
}

fn is_gnome_desktop() -> bool {
    env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.contains("GNOME"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_variety_conf_sets_expected_keys() {
        let existing = "change_interval = 300\ninternet_enabled = True\n[sources]\nsrc1 = True|flickr|foo\n[filters]\nfilter1 = False|Keep original|\n";
        let updated = configure_variety_conf_text(existing, "/pics", 10);
        assert!(updated.contains("change_interval = 600"));
        assert!(updated.contains("internet_enabled = False"));
        assert!(updated.contains("wallpaper_auto_rotate = True"));
        assert!(updated.contains("[sources]\nsrc1 = True|folder|/pics\n"));
        assert!(updated.contains("[filters]"));
    }

    #[test]
    fn desktop_normalization_is_idempotent() {
        let first = normalize_desktop_entry(
            "[Desktop Entry]\nType=Application\nX-GNOME-Autostart-enabled=false\nX-GNOME-Autostart-enabled=true\n",
        );
        let second = normalize_desktop_entry(&first);
        assert_eq!(first, second);

        let count = first
            .lines()
            .filter(|line| line.starts_with("X-GNOME-Autostart-enabled="))
            .count();
        assert_eq!(count, 1);
        assert!(first.contains("X-GNOME-Autostart-enabled=true"));
    }

    #[test]
    fn parses_passwd_entry() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nuser1:x:1000:1000::/home/user1:/bin/bash\n";
        assert_eq!(
            passwd_entry_for_user_from_passwd("user1", passwd),
            Some(PasswdEntry {
                uid: 1000,
                gid: 1000,
                home: PathBuf::from("/home/user1")
            })
        );
        assert_eq!(passwd_entry_for_user_from_passwd("missing", passwd), None);
    }
}
