use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, bail};

pub fn run(node_version: String) -> anyhow::Result<()> {
    super::npm::run(super::npm::Options {
        version: node_version,
    })
    .context("failed to install prerequisite target `npm`")?;

    if let Some(claude) = managed_program("claude") {
        println!("Claude Code already installed:");
        run_command(&claude, &["--version"])?;
        return Ok(());
    }

    install_claude_package()?;

    let Some(claude) = managed_program("claude") else {
        bail!("`claude` was not found on PATH after installation");
    };

    println!("Claude Code installation complete:");
    run_command(&claude, &["--version"])?;

    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let Some(npm) = managed_program("npm") else {
        println!("Claude Code is not installed: managed npm was not found.");
        return Ok(());
    };

    if managed_program("claude").is_none() {
        println!("Claude Code is not installed.");
        return Ok(());
    }

    let prefix = claude_prefix_dir().context("failed to determine per-user npm prefix")?;
    let status = Command::new(&npm)
        .args(["uninstall", "-g", "@anthropic-ai/claude-code"])
        .env("NPM_CONFIG_PREFIX", &prefix)
        .status()
        .with_context(|| format!("failed to start `{}`", npm.display()))?;
    if !status.success() {
        bail!(
            "command `{}` failed with status {}",
            format!("{} uninstall -g @anthropic-ai/claude-code", npm.display()),
            status
        );
    }

    if let Some(claude) = managed_program("claude") {
        if claude.exists() {
            bail!(
                "`claude` still exists after uninstall at {}",
                claude.display()
            );
        }
    }

    println!("Claude Code uninstalled.");
    Ok(())
}

fn install_claude_package() -> anyhow::Result<()> {
    let npm = super::npm::managed_program_path("npm")
        .context("`npm` was not found after installing Node.js")?;
    let prefix = claude_prefix_dir().context("failed to determine per-user npm prefix")?;

    let status = Command::new(&npm)
        .args(["install", "-g", "@anthropic-ai/claude-code"])
        .env("NPM_CONFIG_PREFIX", &prefix)
        .status()
        .with_context(|| format!("failed to start `{}`", npm.display()))?;
    if !status.success() {
        bail!(
            "command `{}` failed with status {}",
            format!("{} install -g @anthropic-ai/claude-code", npm.display()),
            status
        );
    }

    Ok(())
}

fn run_command(program: &PathBuf, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to start `{}`", program.display()))?;
    if !status.success() {
        bail!(
            "command `{}` failed with status {}",
            format!("{} {}", program.display(), args.join(" ")),
            status
        );
    }
    Ok(())
}

fn managed_program(program: &str) -> Option<PathBuf> {
    let path = super::npm::managed_program_path(program).ok()?;
    if path.exists() { Some(path) } else { None }
}

fn claude_prefix_dir() -> anyhow::Result<PathBuf> {
    super::npm::managed_bin_dir().map(|bin_dir| bin_dir.parent().unwrap().to_path_buf())
}
