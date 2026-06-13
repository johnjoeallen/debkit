use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, bail};

use crate::config::SudoNopassConfig;

const SUDOERS_MAIN_PATH: &str = "/etc/sudoers";
const SUDOERS_DROPIN_DIR: &str = "/etc/sudoers.d";
const SUDOERS_STD_GROUP_PATH: &str = "/etc/sudoers.d/00-sudo-group";
const LEGACY_NOPASS_PATH: &str = "/etc/sudoers.d/nopass";

pub fn run(config: &SudoNopassConfig) -> anyhow::Result<()> {
    if !config.enabled {
        println!("Passwordless sudo is disabled (`sudo_nopass.enabled = false`).");
        return Ok(());
    }

    if config.group.trim().is_empty() {
        bail!("`sudo_nopass.group` must not be empty");
    }

    if !config.nis_managed {
        ensure_group_exists(&config.group)?;
    }
    ensure_standard_sudo_rule()?;
    remove_legacy_sudoers_file()?;
    ensure_group_nopass_rule(&config.group)?;
    if !config.nis_managed {
        add_users_to_group(config)?;
    }
    validate_sudoers()?;
    validate_passwordless_sudo(config);

    println!("Passwordless sudo configured:");
    println!("  Group: {}", config.group);
    if config.nis_managed {
        println!("  Mode: NIS-managed (group membership controlled via NIS)");
    } else {
        println!("  Users: {}", effective_users(config).join(", "));
    }
    println!("  Drop-in: {SUDOERS_DROPIN_DIR}/99-{}-nopass", config.group);
    Ok(())
}

fn ensure_group_exists(group: &str) -> anyhow::Result<()> {
    if local_group_exists(group) {
        return Ok(());
    }
    if group_exists(group) {
        bail!(
            "`sudo_nopass.group = \"{group}\"` resolves through NSS but is not a local /etc/group entry; set `sudo_nopass.nis_managed = true` for NIS-managed group membership or choose a local group"
        );
    }
    run_root_command("groupadd", &[group])?;
    Ok(())
}

fn ensure_standard_sudo_rule() -> anyhow::Result<()> {
    let has_rule = fs::read_to_string(SUDOERS_MAIN_PATH)
        .ok()
        .map(|content| {
            content.lines().any(|line| {
                line.trim() == "%sudo ALL=(ALL:ALL) ALL"
                    || line.trim() == "%sudo ALL=(ALL) ALL"
                    || line.trim() == "%sudo\tALL=(ALL:ALL) ALL"
            })
        })
        .unwrap_or(false);
    if has_rule {
        return Ok(());
    }

    ensure_root_file(SUDOERS_STD_GROUP_PATH, "%sudo ALL=(ALL:ALL) ALL\n")?;
    secure_sudoers_dropin(SUDOERS_STD_GROUP_PATH)
}

fn remove_legacy_sudoers_file() -> anyhow::Result<()> {
    if !Path::new(LEGACY_NOPASS_PATH).exists() {
        return Ok(());
    }
    run_root_command("rm", &[LEGACY_NOPASS_PATH])?;
    Ok(())
}

fn ensure_group_nopass_rule(group: &str) -> anyhow::Result<()> {
    let path = format!("{SUDOERS_DROPIN_DIR}/99-{}-nopass", group);
    let content = render_group_nopass_rule(group);
    ensure_root_file(&path, &content)?;
    secure_sudoers_dropin(&path)
}

fn secure_sudoers_dropin(path: &str) -> anyhow::Result<()> {
    run_root_command("chown", &["root:root", path])?;
    run_root_command("chmod", &["0440", &path])
}

fn render_group_nopass_rule(group: &str) -> String {
    format!("%{group} ALL=(ALL:ALL) NOPASSWD: ALL\n")
}

fn add_users_to_group(config: &SudoNopassConfig) -> anyhow::Result<()> {
    let mut users = effective_users(config);
    users.sort();
    users.dedup();

    for user in users {
        if !user_exists(&user) {
            println!("Skipping user {user}; account not found.");
            continue;
        }
        if user_is_in_group(&user, &config.group)? {
            continue;
        }
        run_root_command("usermod", &["-aG", &config.group, &user])?;
    }

    Ok(())
}

fn validate_sudoers() -> anyhow::Result<()> {
    let status = Command::new("visudo")
        .arg("-c")
        .status()
        .context("failed to run visudo")?;
    if !status.success() {
        bail!("visudo -c failed with status {}", status);
    }
    Ok(())
}

fn validate_passwordless_sudo(config: &SudoNopassConfig) {
    let group = config.group.trim();
    if group.is_empty() {
        return;
    }

    match Command::new("getent").args(["group", group]).output() {
        Ok(output) if output.status.success() => {
            let raw = String::from_utf8_lossy(&output.stdout);
            println!("Validated sudo group lookup:");
            println!("  getent group {group}");
            if !raw
                .lines()
                .any(|line| line.starts_with(&format!("{group}:")))
            {
                println!("warning: `getent group {group}` succeeded but did not return `{group}`.");
            }
        }
        Ok(output) => {
            println!(
                "warning: `getent group {group}` failed with status {}.",
                output.status
            );
        }
        Err(err) => {
            println!("warning: failed to run `getent group {group}`: {err}.");
        }
    }

    for user in effective_users(config) {
        match user_is_in_group(&user, group) {
            Ok(true) => {}
            Ok(false) => {
                println!(
                    "warning: `{user}` is not currently reported as a member of `{group}` by `id -nG {user}`; sudo will ask for a password until NSS reports that membership."
                );
            }
            Err(err) => {
                println!("warning: failed to check `id -nG {user}`: {err:#}");
            }
        }

        if current_user().as_deref() == Some(user.as_str()) && !current_process_is_in_group(group) {
            println!(
                "warning: the current login session is not in `{group}` according to plain `id`; start a new login session before testing passwordless sudo."
            );
        }

        if current_euid().ok() == Some(0) {
            match sudo_policy_allows_nopass(&user) {
                Ok(true) => {}
                Ok(false) => {
                    println!(
                        "warning: `sudo -n -l -U {user}` does not show a NOPASSWD rule; `sudo -i` will still ask for a password."
                    );
                }
                Err(err) => {
                    println!("warning: failed to validate sudo policy for `{user}`: {err:#}");
                }
            }
        } else {
            println!(
                "warning: skipping exact sudo policy validation for `{user}` because DebKit is not running as root."
            );
        }
    }
}

fn sudo_policy_allows_nopass(user: &str) -> anyhow::Result<bool> {
    let output = Command::new("sudo")
        .args(["-n", "-l", "-U", user])
        .output()
        .with_context(|| format!("failed to run `sudo -n -l -U {user}`"))?;
    if !output.status.success() {
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(stdout.contains("NOPASSWD") || stderr.contains("NOPASSWD"))
}

fn effective_users(config: &SudoNopassConfig) -> Vec<String> {
    let mut users: Vec<String> = config
        .users
        .iter()
        .filter_map(|user| {
            let trimmed = user.trim();
            (!trimmed.is_empty() && trimmed != "root").then(|| trimmed.to_string())
        })
        .collect();

    if config.add_current_user {
        if let Some(user) = current_user() {
            if user != "root" {
                users.push(user);
            }
        }
    }

    users
}

fn current_user() -> Option<String> {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("USER").ok())
        .map(|value| value.trim().to_string())
}

fn group_exists(group: &str) -> bool {
    Command::new("getent")
        .args(["group", group])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn local_group_exists(group: &str) -> bool {
    fs::read_to_string("/etc/group")
        .ok()
        .map(|raw| {
            raw.lines()
                .any(|line| line.starts_with(&format!("{group}:")))
        })
        .unwrap_or(false)
}

fn user_exists(user: &str) -> bool {
    Command::new("id")
        .args(["-u", user])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn user_is_in_group(user: &str, group: &str) -> anyhow::Result<bool> {
    let output = Command::new("id")
        .args(["-nG", user])
        .output()
        .with_context(|| format!("failed to check group membership for {user}"))?;
    if !output.status.success() {
        return Ok(false);
    }
    let groups = String::from_utf8_lossy(&output.stdout);
    Ok(groups.split_whitespace().any(|item| item == group))
}

fn current_process_is_in_group(group: &str) -> bool {
    Command::new("id")
        .arg("-nG")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .any(|item| item == group)
        })
        .unwrap_or(false)
}

fn run_root_command(program: &str, args: &[&str]) -> anyhow::Result<()> {
    if current_euid()? == 0 {
        let status = Command::new(program)
            .args(args)
            .status()
            .with_context(|| format!("failed to start `{program}`"))?;
        if !status.success() {
            bail!("{program} {} failed with status {}", args.join(" "), status);
        }
        return Ok(());
    }

    if !command_available("sudo") {
        bail!("`{program}` requires root privileges; run as root or install `sudo` and retry");
    }

    let status = Command::new("sudo")
        .arg(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to start `sudo {program}`"))?;
    if !status.success() {
        bail!(
            "sudo {program} {} failed with status {}",
            args.join(" "),
            status
        );
    }
    Ok(())
}

fn ensure_root_file(path: &str, content: &str) -> anyhow::Result<()> {
    if Path::new(path).exists() {
        let existing = fs::read_to_string(path).unwrap_or_default();
        if existing == content {
            return Ok(());
        }
    }

    if current_euid()? == 0 {
        fs::write(path, content).with_context(|| format!("failed to write {path}"))?;
        return Ok(());
    }

    if !command_available("sudo") {
        bail!("writing {path} requires root privileges; run as root or install `sudo` and retry");
    }

    let status = Command::new("sudo")
        .arg("tee")
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(content.as_bytes())?;
            child.wait()
        })
        .with_context(|| format!("failed to write {path}"))?;
    if !status.success() {
        bail!("sudo tee {path} failed with status {status}");
    }
    Ok(())
}

fn command_available(program: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {program} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn current_euid() -> anyhow::Result<u32> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to run `id -u`")?;
    if !output.status.success() {
        bail!("`id -u` failed with status {}", output.status);
    }

    let stdout = String::from_utf8(output.stdout).context("`id -u` returned non-UTF-8 output")?;
    stdout
        .trim()
        .parse::<u32>()
        .with_context(|| format!("failed to parse `id -u` output `{}`", stdout.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nopass_rule_allows_runas_user_and_group() {
        assert_eq!(
            render_group_nopass_rule("superuser"),
            "%superuser ALL=(ALL:ALL) NOPASSWD: ALL\n"
        );
    }
}
