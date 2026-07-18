use std::process::Command;

use anyhow::{Context, bail};

pub fn install_missing(packages: &[&str]) -> anyhow::Result<Vec<String>> {
    let missing = missing_packages(packages)?;
    if missing.is_empty() {
        return Ok(missing);
    }

    run(&["update"])?;

    let mut args = vec!["install", "-y"];
    args.extend(missing.iter().map(String::as_str));
    run(&args)?;

    Ok(missing)
}

pub fn remove(packages: &[&str]) -> anyhow::Result<()> {
    let mut args = vec!["remove", "-y"];
    args.extend(packages.iter().copied());
    run(&args)
}

pub fn package_installed(package: &str) -> anyhow::Result<bool> {
    let status = Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", package])
        .output()
        .with_context(|| format!("failed to query package `{package}`"))?;

    if !status.status.success() {
        return Ok(false);
    }

    let stdout = String::from_utf8_lossy(&status.stdout);
    Ok(stdout.trim().eq_ignore_ascii_case("install ok installed"))
}

fn missing_packages(packages: &[&str]) -> anyhow::Result<Vec<String>> {
    let mut missing = Vec::new();
    for package in packages {
        if !package_installed(package)? {
            missing.push((*package).to_string());
        }
    }
    Ok(missing)
}

fn run(args: &[&str]) -> anyhow::Result<()> {
    let euid = current_euid()?;

    let mut command;
    if euid == 0 {
        command = Command::new("apt-get");
        command.args(args);
    } else if command_available("sudo") {
        command = Command::new("sudo");
        command.arg("apt-get").args(args);
    } else {
        bail!("apt operations require root privileges; run as root or install `sudo` and retry");
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

fn command_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
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
    let trimmed = stdout.trim();
    trimmed
        .parse::<u32>()
        .with_context(|| format!("failed to parse `id -u` output `{trimmed}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_package_status_is_detected() {
        assert!(package_installed("definitely-not-a-real-debkit-package-name").is_ok());
    }
}
