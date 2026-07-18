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
    super::apt::install_missing(&["git"])?;
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
