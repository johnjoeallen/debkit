use std::process::Command;

use anyhow::{Context, bail};

pub fn run() -> anyhow::Result<()> {
    if command_available("git") {
        println!("git already installed:");
        run_command("git", &["--version"])?;
        return Ok(());
    }

    install_git_package()?;

    if !command_available("git") {
        bail!("`git` was not found on PATH after installation");
    }

    println!("git installation complete:");
    run_command("git", &["--version"])?;

    Ok(())
}

fn install_git_package() -> anyhow::Result<()> {
    run_apt_command(&["update"])?;
    run_apt_command(&["install", "-y", "git"])?;
    Ok(())
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
        bail!("installing git requires root privileges; run as root or install `sudo` and retry");
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

fn run_command(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to start `{program}`"))?;
    if !status.success() {
        bail!(
            "command `{}` failed with status {}",
            format!("{program} {}", args.join(" ")),
            status
        );
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
