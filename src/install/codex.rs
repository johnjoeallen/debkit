use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, bail};

pub fn run(node_version: String) -> anyhow::Result<()> {
    super::npm::run(super::npm::Options {
        version: node_version,
    })
    .context("failed to install prerequisite target `npm`")?;

    if let Some(codex) = managed_program("codex") {
        println!("Codex already installed:");
        run_command(&codex, &["--version"])?;
        return Ok(());
    }

    install_codex_package()?;

    let Some(codex) = managed_program("codex") else {
        bail!("`codex` was not found on PATH after installation");
    };

    println!("Codex installation complete:");
    run_command(&codex, &["--version"])?;

    Ok(())
}

fn install_codex_package() -> anyhow::Result<()> {
    let npm = super::npm::managed_program_path("npm")
        .context("`npm` was not found after installing Node.js")?;
    let prefix = codex_prefix_dir().context("failed to determine per-user npm prefix")?;

    let status = Command::new(&npm)
        .args(["install", "-g", "@openai/codex"])
        .env("NPM_CONFIG_PREFIX", &prefix)
        .status()
        .with_context(|| format!("failed to start `{}`", npm.display()))?;
    if !status.success() {
        bail!(
            "command `{}` failed with status {}",
            format!("{} install -g @openai/codex", npm.display()),
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

fn codex_prefix_dir() -> anyhow::Result<PathBuf> {
    super::npm::managed_bin_dir().map(|bin_dir| bin_dir.parent().unwrap().to_path_buf())
}
