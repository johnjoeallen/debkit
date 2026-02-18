use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, bail};

#[derive(Debug, Clone)]
pub struct Options {
    pub reinstall: bool,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    ensure_shell_init_sources_cargo_env()?;

    if !options.reinstall && command_available("cargo") && command_available("rustc") {
        println!("Rust already installed:");
        run_command("cargo", &["--version"])?;
        run_command("rustc", &["--version"])?;
        return Ok(());
    }

    if command_available("rustup") {
        if options.reinstall {
            run_command("rustup", &["self", "update"])?;
        }
        run_command("rustup", &["toolchain", "install", "stable"])?;
        run_command("rustup", &["default", "stable"])?;
    } else {
        run_shell_command(
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
             sh -s -- -y --profile default --default-toolchain stable",
        )?;
    }

    ensure_shell_init_sources_cargo_env()?;
    println!("Rust installation complete:");
    run_command("cargo", &["--version"])?;
    run_command("rustc", &["--version"])?;

    Ok(())
}

fn ensure_shell_init_sources_cargo_env() -> anyhow::Result<()> {
    let home = home_dir()?;
    let line = r#"source "$HOME/.cargo/env""#;
    let files = [home.join(".bashrc"), home.join(".profile")];

    for file in files {
        if !file.exists() {
            fs::write(&file, "").with_context(|| format!("failed to create {}", file.display()))?;
        }

        let content = fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        if content.lines().any(|existing| existing.trim() == line) {
            continue;
        }

        let mut handle = OpenOptions::new()
            .append(true)
            .open(&file)
            .with_context(|| format!("failed to open {} for append", file.display()))?;
        writeln!(handle)?;
        writeln!(handle, "{line}")?;
    }

    Ok(())
}

fn command_available(program: &str) -> bool {
    resolve_program(program).is_some()
}

fn run_command(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let Some(program_path) = resolve_program(program) else {
        bail!("`{program}` executable was not found in PATH or ~/.cargo/bin");
    };

    let status = Command::new(&program_path)
        .args(args)
        .status()
        .with_context(|| format!("failed to start `{}`", program_path.display()))?;

    if !status.success() {
        bail!(
            "command `{}` failed with status {}",
            format!("{} {}", program_path.display(), args.join(" ")),
            status
        );
    }

    Ok(())
}

fn run_shell_command(cmd: &str) -> anyhow::Result<()> {
    let status = Command::new("sh")
        .args(["-c", cmd])
        .status()
        .context("failed to start shell command")?;
    if !status.success() {
        bail!("shell command failed with status {}", status);
    }
    Ok(())
}

fn resolve_program(program: &str) -> Option<PathBuf> {
    if Command::new(program)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
    {
        return Some(PathBuf::from(program));
    }

    let cargo_bin = home_dir().ok()?.join(".cargo").join("bin").join(program);
    if cargo_bin.exists() {
        return Some(cargo_bin);
    }

    None
}

fn home_dir() -> anyhow::Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}
