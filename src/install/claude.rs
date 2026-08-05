use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, bail};

const INSTALL_SCRIPT_URL: &str = "https://claude.ai/install.sh";

pub fn run(version: String) -> anyhow::Result<()> {
    let version = version.trim();
    if version.is_empty() {
        bail!("Claude Code version/channel must not be empty");
    }

    if let Some(claude) = managed_program() {
        println!("Claude Code already installed:");
        run_command(&claude, &["--version"])?;
        return Ok(());
    }

    install_native(version)?;

    let Some(claude) = managed_program() else {
        bail!("`claude` was not found at ~/.local/bin/claude after installation");
    };

    println!("Claude Code installation complete:");
    run_command(&claude, &["--version"])?;

    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let home = home_dir()?;
    let claude_bin = managed_program_path_for_home(&home);
    let versions_dir = home.join(".local").join("share").join("claude");

    let mut removed_any = false;

    if claude_bin.exists() || fs::symlink_metadata(&claude_bin).is_ok() {
        fs::remove_file(&claude_bin)
            .with_context(|| format!("failed to remove {}", claude_bin.display()))?;
        removed_any = true;
    }

    if versions_dir.exists() {
        fs::remove_dir_all(&versions_dir)
            .with_context(|| format!("failed to remove {}", versions_dir.display()))?;
        removed_any = true;
    }

    if !removed_any {
        println!("Claude Code is not installed.");
        return Ok(());
    }

    println!("Claude Code uninstalled.");
    Ok(())
}

/// Runs the equivalent of `curl -fsSL https://claude.ai/install.sh | bash -s <version>`
/// without ever handing a shell an interpolated string, so `version` can't be used for
/// shell injection: it's piped to `bash` as a plain argv element, which is how the
/// upstream install docs pass a release channel or specific version through to the
/// script's own positional-parameter handling.
fn install_native(version: &str) -> anyhow::Result<()> {
    let mut curl = Command::new("curl")
        .args(["-fsSL", INSTALL_SCRIPT_URL])
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start `curl -fsSL {INSTALL_SCRIPT_URL}`"))?;
    let curl_stdout = curl
        .stdout
        .take()
        .context("failed to capture curl output")?;

    let mut bash = Command::new("bash");
    bash.arg("-s");
    if !version.eq_ignore_ascii_case("latest") {
        bash.arg(version);
    }
    let bash_status = bash
        .stdin(Stdio::from(curl_stdout))
        .status()
        .context("failed to start `bash`")?;

    let curl_status = curl.wait().context("failed to wait for `curl`")?;
    if !curl_status.success() {
        bail!("`curl -fsSL {INSTALL_SCRIPT_URL}` failed with status {curl_status}");
    }
    if !bash_status.success() {
        bail!("Claude Code install script failed with status {bash_status}");
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

fn managed_program() -> Option<PathBuf> {
    let path = managed_program_path_for_home(&home_dir().ok()?);
    if path.exists() { Some(path) } else { None }
}

fn managed_program_path_for_home(home: &std::path::Path) -> PathBuf {
    home.join(".local").join("bin").join("claude")
}

fn home_dir() -> anyhow::Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}
