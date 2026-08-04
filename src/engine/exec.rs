use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, bail};

pub fn current_euid() -> anyhow::Result<u32> {
    let status = fs::read_to_string("/proc/self/status").context("failed to read effective uid")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let mut fields = rest.split_whitespace();
            let _real = fields.next();
            let Some(effective) = fields.next() else {
                break;
            };
            return effective
                .parse::<u32>()
                .context("failed to parse effective uid");
        }
    }
    bail!("failed to determine effective uid")
}

pub fn command_available(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn capture(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    if !output.status.success() {
        bail!(
            "{} {} failed with status {}",
            program,
            args.join(" "),
            output.status
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Runs `program` directly when already root, otherwise through `sudo`.
pub fn run_privileged(program: &str, args: &[&str]) -> anyhow::Result<()> {
    run_privileged_with_input(program, args, "")
}

/// Runs `program` directly as the invoking user — never through `sudo`. Required for
/// per-user state (`git config --global`, files under the invoking user's `$HOME`) where
/// running as root would silently operate on the wrong home directory.
pub fn run_as_current_user(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to launch {program}"))?;
    if !status.success() {
        bail!(
            "{} {} failed with status {}",
            program,
            args.join(" "),
            status
        );
    }
    Ok(())
}

pub fn run_privileged_with_input(program: &str, args: &[&str], stdin: &str) -> anyhow::Result<()> {
    let euid = current_euid()?;

    let mut command;
    if euid == 0 {
        command = Command::new(program);
        command.args(args);
    } else if command_available("sudo") {
        command = Command::new("sudo");
        command.arg(program).args(args);
    } else {
        bail!("`{program}` requires root privileges; run as root or install `sudo` and retry");
    }

    if stdin.is_empty() {
        let status = command
            .status()
            .with_context(|| format!("failed to launch {program}"))?;
        if !status.success() {
            bail!(
                "{} {} failed with status {}",
                program,
                args.join(" "),
                status
            );
        }
        return Ok(());
    }

    let mut child = command
        .stdin(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch {program}"))?;
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("piped stdin")
            .write_all(stdin.as_bytes())
            .with_context(|| format!("failed to write stdin to {program}"))?;
    }
    drop(child.stdin.take());
    let status = child
        .wait()
        .with_context(|| format!("failed while running {program}"))?;
    if !status.success() {
        bail!(
            "{} {} failed with status {}",
            program,
            args.join(" "),
            status
        );
    }
    Ok(())
}

/// Creates `path` (and parents) as root/sudo if it does not already exist.
/// Returns whether the directory was created.
pub fn ensure_root_dir(path: &Path) -> anyhow::Result<bool> {
    if path.is_dir() {
        return Ok(false);
    }
    if current_euid()? == 0 {
        fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
        return Ok(true);
    }
    if !command_available("sudo") {
        bail!(
            "creating {} requires root privileges; run as root or install `sudo` and retry",
            path.display()
        );
    }
    let status = Command::new("sudo")
        .arg("mkdir")
        .arg("-p")
        .arg(path)
        .status()
        .with_context(|| format!("failed to create {}", path.display()))?;
    if !status.success() {
        bail!(
            "sudo mkdir -p {} failed with status {}",
            path.display(),
            status
        );
    }
    Ok(true)
}

/// Writes `content` to `path` as root/sudo only if it differs from the existing content.
/// Returns whether the file was written.
pub fn ensure_root_file(path: &Path, content: &str) -> anyhow::Result<bool> {
    if path.exists() {
        let existing = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if existing == content {
            return Ok(false);
        }
    }

    if current_euid()? == 0 {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
        return Ok(true);
    }

    if !command_available("sudo") {
        bail!(
            "writing {} requires root privileges; run as root or install `sudo` and retry",
            path.display()
        );
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
        .with_context(|| format!("failed to write {}", path.display()))?;
    if !status.success() {
        bail!("sudo tee {} failed with status {}", path.display(), status);
    }
    Ok(true)
}

/// Removes `path` as root/sudo if it exists. Used to undo a `WriteFile` change whose
/// rollback snapshot recorded that the file did not exist beforehand.
pub fn remove_root_file(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if current_euid()? == 0 {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;
        return Ok(());
    }
    if !command_available("sudo") {
        bail!(
            "removing {} requires root privileges; run as root or install `sudo` and retry",
            path.display()
        );
    }
    let status = Command::new("sudo")
        .arg("rm")
        .arg("-f")
        .arg(path)
        .status()
        .with_context(|| format!("failed to remove {}", path.display()))?;
    if !status.success() {
        bail!(
            "sudo rm -f {} failed with status {}",
            path.display(),
            status
        );
    }
    Ok(())
}

/// Reads the current content of `path`, if it exists. Used to snapshot pre-change state
/// for rollback.
pub fn read_file_if_exists(path: &Path) -> anyhow::Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(fs::read_to_string(path).with_context(|| {
        format!("failed to read {}", path.display())
    })?))
}

pub fn apt_update_install(packages: &[&str]) -> anyhow::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    run_apt_command(&["update"])?;
    let mut args = vec!["install", "-y"];
    args.extend(packages);
    run_apt_command(&args)?;
    for package in packages {
        if !package_installed(package)? {
            bail!("`{package}` was not installed");
        }
    }
    Ok(())
}

/// Used by `engine::apply::rollback` to undo `InstallPackages`, and only ever called
/// with the subset of packages that weren't already present before `apply` — never a
/// package the user already had for unrelated reasons.
pub fn apt_remove(packages: &[&str]) -> anyhow::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    let mut args = vec!["remove", "-y"];
    args.extend(packages);
    run_apt_command(&args)
}

fn run_apt_command(args: &[&str]) -> anyhow::Result<()> {
    let euid = current_euid()?;
    let mut command;
    if euid == 0 {
        command = Command::new("apt");
        command.args(args);
    } else if command_available("sudo") {
        command = Command::new("sudo");
        command.arg("apt").args(args);
    } else {
        bail!(
            "installing packages requires root privileges; run as root or install `sudo` and retry"
        );
    }

    let status = command
        .env("DEBIAN_FRONTEND", "noninteractive")
        .status()
        .context("failed to launch apt")?;
    if !status.success() {
        bail!("apt {} failed with status {}", args.join(" "), status);
    }
    Ok(())
}

pub(crate) fn package_installed(package: &str) -> anyhow::Result<bool> {
    let status = Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", package])
        .status()
        .with_context(|| format!("failed to query package `{package}`"))?;
    Ok(status.success())
}

pub fn systemctl_is_enabled(unit: &str) -> bool {
    Command::new("systemctl")
        .arg("is-enabled")
        .arg("--quiet")
        .arg(unit)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn systemctl_is_active(unit: &str) -> bool {
    Command::new("systemctl")
        .arg("is-active")
        .arg("--quiet")
        .arg(unit)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn systemd_unit_exists(unit: &str) -> bool {
    Path::new(&format!("/usr/lib/systemd/system/{unit}")).exists()
        || Path::new(&format!("/etc/systemd/system/{unit}")).exists()
}

pub fn enable_and_start_service(unit: &str) -> anyhow::Result<()> {
    run_privileged("systemctl", &["enable", "--now", unit])
}

pub fn restart_service(unit: &str) -> anyhow::Result<()> {
    run_privileged("systemctl", &["restart", unit])
}

/// Used by `engine::apply::rollback` to undo `ServiceAction::EnableNow` when the unit
/// wasn't already enabled beforehand.
pub fn disable_service(unit: &str) -> anyhow::Result<()> {
    run_privileged("systemctl", &["disable", unit])
}

/// Used by `engine::apply::rollback` to undo `ServiceAction::EnableNow` when the unit
/// wasn't already active beforehand.
pub fn stop_service(unit: &str) -> anyhow::Result<()> {
    run_privileged("systemctl", &["stop", unit])
}

pub fn current_hostname() -> anyhow::Result<String> {
    Ok(capture("hostname", &[])?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_available_finds_sh_builtins() {
        assert!(command_available("ls"));
        assert!(!command_available("debkit-definitely-not-a-real-command"));
    }

    #[test]
    fn current_euid_reads_proc_status() {
        // Should not error under any test runner; value itself is environment-dependent.
        assert!(current_euid().is_ok());
    }
}
