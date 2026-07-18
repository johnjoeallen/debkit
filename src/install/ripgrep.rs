use std::process::Command;

use anyhow::{Context, bail};

pub fn run() -> anyhow::Result<()> {
    if command_available("rg") {
        run_command("rg", &["--version"])?;
        return Ok(());
    }

    install_ripgrep_package()?;

    if !command_available("rg") {
        bail!("`rg` was not found on PATH after installation");
    }

    println!("ripgrep installation complete:");
    run_command("rg", &["--version"])?;

    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    if !command_available("rg") {
        println!("ripgrep is not installed.");
        return Ok(());
    }

    super::apt::remove(&["ripgrep"])?;

    if command_available("rg") {
        bail!("`rg` is still available on PATH after uninstall");
    }

    println!("ripgrep uninstalled.");
    Ok(())
}

fn install_ripgrep_package() -> anyhow::Result<()> {
    super::apt::install_missing(&["ripgrep"])?;
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
