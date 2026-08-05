use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

const NODE_BASE_URL: &str = "https://nodejs.org/dist";
const LOCAL_BIN_PATH_LINE: &str = r#"export PATH="$HOME/.local/bin:$PATH""#;

#[derive(Debug, Clone)]
pub struct Options {
    pub version: String,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    ensure_shell_init_sources_local_bin()?;

    let install_spec = install_spec(&options.version)?;
    let home = home_dir()?;

    if managed_install_exists_for(&home, &install_spec.version_dir_name) {
        println!(
            "npm already installed from upstream Node.js binaries ({}):",
            install_spec.display_version
        );
        run_command(
            managed_program_path_for_home(&home, "node")
                .to_string_lossy()
                .as_ref(),
            &["--version"],
        )?;
        run_command(
            managed_program_path_for_home(&home, "npm")
                .to_string_lossy()
                .as_ref(),
            &["--version"],
        )?;
        return Ok(());
    }

    install_nodejs_from_upstream(&home, &install_spec)?;

    if !managed_install_exists_for(&home, &install_spec.version_dir_name) {
        bail!(
            "DebKit-managed Node.js install `{}` was not found after installation",
            install_spec.display_version
        );
    }

    println!("npm installation complete:");
    run_command(
        managed_program_path_for_home(&home, "node")
            .to_string_lossy()
            .as_ref(),
        &["--version"],
    )?;
    run_command(
        managed_program_path_for_home(&home, "npm")
            .to_string_lossy()
            .as_ref(),
        &["--version"],
    )?;

    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let home = home_dir()?;
    let install_root = managed_install_root_for_home(&home);
    let current_link = managed_current_link_for_home(&home);
    let local_prefix = managed_prefix_dir_for_home(&home);
    let local_node_modules = local_prefix.join("lib").join("node_modules");
    let managed_bins = [
        managed_program_path_for_home(&home, "node"),
        managed_program_path_for_home(&home, "npm"),
        managed_program_path_for_home(&home, "npx"),
        managed_program_path_for_home(&home, "codex"),
    ];

    let mut removed_any = false;

    for bin in managed_bins {
        if bin.exists() || fs::symlink_metadata(&bin).is_ok() {
            fs::remove_file(&bin).with_context(|| format!("failed to remove {}", bin.display()))?;
            removed_any = true;
        }
    }

    if fs::symlink_metadata(&current_link).is_ok() {
        fs::remove_file(&current_link)
            .with_context(|| format!("failed to remove {}", current_link.display()))?;
        removed_any = true;
    }

    if install_root.exists() {
        fs::remove_dir_all(&install_root)
            .with_context(|| format!("failed to remove {}", install_root.display()))?;
        removed_any = true;
    }

    if local_node_modules.exists() {
        fs::remove_dir_all(&local_node_modules)
            .with_context(|| format!("failed to remove {}", local_node_modules.display()))?;
        removed_any = true;
    }

    if !removed_any {
        println!("npm is not installed.");
        return Ok(());
    }

    println!("npm uninstalled.");
    Ok(())
}

#[derive(Debug, Clone)]
struct InstallSpec {
    release_path: String,
    version_dir_name: String,
    display_version: String,
}

fn install_spec(version: &str) -> anyhow::Result<InstallSpec> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        bail!("Node.js version must not be empty");
    }

    if trimmed.eq_ignore_ascii_case("latest") {
        let arch = node_arch()?;
        let shasums_url = format!("{NODE_BASE_URL}/latest/SHASUMS256.txt");
        let shasums = run_capture_command("curl", &["-fsSL", &shasums_url])
            .with_context(|| format!("failed to download {shasums_url}"))?;
        let archive_name = parse_archive_name(&shasums, arch)?;
        let version_dir_name = archive_name
            .strip_suffix(".tar.xz")
            .context("unexpected Node.js archive filename in SHASUMS256.txt")?
            .to_string();
        return Ok(InstallSpec {
            release_path: "latest".to_string(),
            display_version: version_dir_name.trim_start_matches("node-").to_string(),
            version_dir_name,
        });
    }

    let normalized = normalize_version(trimmed);
    Ok(InstallSpec {
        release_path: normalized.clone(),
        version_dir_name: format!("node-{normalized}"),
        display_version: normalized,
    })
}

fn normalize_version(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

fn install_nodejs_from_upstream(home: &Path, install_spec: &InstallSpec) -> anyhow::Result<()> {
    let arch = node_arch()?;
    let shasums_url = format!(
        "{NODE_BASE_URL}/{}/SHASUMS256.txt",
        install_spec.release_path
    );
    let shasums = run_capture_command("curl", &["-fsSL", &shasums_url])
        .with_context(|| format!("failed to download {shasums_url}"))?;
    let archive_name = parse_archive_name(&shasums, arch)?;

    let tmp_dir = managed_tmp_dir_for_home(home);
    fs::create_dir_all(&tmp_dir)
        .with_context(|| format!("failed to create {}", tmp_dir.display()))?;

    let archive_path = tmp_dir.join(&archive_name);
    let archive_url = format!(
        "{NODE_BASE_URL}/{}/{archive_name}",
        install_spec.release_path
    );
    run_command(
        "curl",
        &[
            "-fsSL",
            &archive_url,
            "-o",
            archive_path.to_string_lossy().as_ref(),
        ],
    )
    .with_context(|| format!("failed to download {archive_url}"))?;

    let extract_dir = tmp_dir.join("extract");
    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir)
            .with_context(|| format!("failed to reset {}", extract_dir.display()))?;
    }
    fs::create_dir_all(&extract_dir)
        .with_context(|| format!("failed to create {}", extract_dir.display()))?;

    run_command(
        "tar",
        &[
            "-xJf",
            archive_path.to_string_lossy().as_ref(),
            "-C",
            extract_dir.to_string_lossy().as_ref(),
        ],
    )
    .context("failed to extract Node.js archive")?;

    let version_dir = extract_dir.join(&install_spec.version_dir_name);
    if !version_dir.exists() {
        bail!(
            "expected extracted Node.js directory `{}` was not found",
            version_dir.display()
        );
    }

    let install_root = managed_install_root_for_home(home);
    let installed_version_dir = install_root.join(&install_spec.version_dir_name);
    let current_link = managed_current_link_for_home(home);
    let local_bin_dir = managed_bin_dir_for_home(home);

    fs::create_dir_all(&install_root)
        .with_context(|| format!("failed to create {}", install_root.display()))?;
    fs::create_dir_all(&local_bin_dir)
        .with_context(|| format!("failed to create {}", local_bin_dir.display()))?;

    if installed_version_dir.exists() {
        fs::remove_dir_all(&installed_version_dir)
            .with_context(|| format!("failed to reset {}", installed_version_dir.display()))?;
    }
    run_command(
        "cp",
        &[
            "-a",
            version_dir.to_string_lossy().as_ref(),
            install_root.to_string_lossy().as_ref(),
        ],
    )?;

    ensure_symlink(&current_link, &installed_version_dir)?;
    ensure_symlink(
        &managed_program_path_for_home(home, "node"),
        &current_link.join("bin").join("node"),
    )?;
    ensure_symlink(
        &managed_program_path_for_home(home, "npm"),
        &current_link.join("bin").join("npm"),
    )?;
    ensure_symlink(
        &managed_program_path_for_home(home, "npx"),
        &current_link.join("bin").join("npx"),
    )?;

    fs::remove_dir_all(&tmp_dir)
        .with_context(|| format!("failed to clean up {}", tmp_dir.display()))?;

    Ok(())
}

fn ensure_shell_init_sources_local_bin() -> anyhow::Result<()> {
    let home = home_dir()?;
    let files = [home.join(".bashrc"), home.join(".profile")];

    for file in files {
        if !file.exists() {
            fs::write(&file, "").with_context(|| format!("failed to create {}", file.display()))?;
        }

        let content = fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        if content
            .lines()
            .any(|existing| existing.trim() == LOCAL_BIN_PATH_LINE)
        {
            continue;
        }

        let mut handle = OpenOptions::new()
            .append(true)
            .open(&file)
            .with_context(|| format!("failed to open {} for append", file.display()))?;
        writeln!(handle)?;
        writeln!(handle, "{LOCAL_BIN_PATH_LINE}")?;
    }

    Ok(())
}

fn ensure_symlink(link: &Path, target: &Path) -> anyhow::Result<()> {
    if let Ok(existing_target) = fs::read_link(link) {
        if existing_target == target {
            return Ok(());
        }
        fs::remove_file(link).with_context(|| format!("failed to remove {}", link.display()))?;
    } else if link.exists() {
        bail!(
            "refusing to overwrite existing non-symlink path `{}`",
            link.display()
        );
    }

    symlink(target, link).with_context(|| {
        format!(
            "failed to create symlink {} -> {}",
            link.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn managed_install_exists_for(home: &Path, version_dir_name: &str) -> bool {
    let current_link = managed_current_link_for_home(home);
    current_link.join("bin/node").exists()
        && fs::read_link(&current_link)
            .map(|target| target == managed_install_root_for_home(home).join(version_dir_name))
            .unwrap_or(false)
        && managed_program_path_for_home(home, "node").exists()
        && managed_program_path_for_home(home, "npm").exists()
}

fn managed_install_root_for_home(home: &Path) -> PathBuf {
    home.join(".local")
        .join("share")
        .join("debkit")
        .join("nodejs")
}

fn managed_prefix_dir_for_home(home: &Path) -> PathBuf {
    home.join(".local")
}

fn managed_tmp_dir_for_home(home: &Path) -> PathBuf {
    home.join("tmp").join("debkit-nodejs")
}

fn managed_current_link_for_home(home: &Path) -> PathBuf {
    managed_install_root_for_home(home).join("current")
}

pub fn managed_bin_dir() -> anyhow::Result<PathBuf> {
    Ok(managed_bin_dir_for_home(&home_dir()?))
}

fn managed_bin_dir_for_home(home: &Path) -> PathBuf {
    home.join(".local").join("bin")
}

pub fn managed_program_path(program: &str) -> anyhow::Result<PathBuf> {
    Ok(managed_program_path_for_home(&home_dir()?, program))
}

fn managed_program_path_for_home(home: &Path, program: &str) -> PathBuf {
    managed_bin_dir_for_home(home).join(program)
}

fn node_arch() -> anyhow::Result<&'static str> {
    let arch = run_capture_command("uname", &["-m"]).context("failed to detect architecture")?;
    match arch.trim() {
        "x86_64" => Ok("x64"),
        "aarch64" => Ok("arm64"),
        other => bail!("unsupported architecture `{other}` for upstream Node.js install"),
    }
}

fn parse_archive_name(shasums: &str, arch: &str) -> anyhow::Result<String> {
    let suffix = format!("-linux-{arch}.tar.xz");
    for line in shasums.lines() {
        let Some((_, filename)) = line.split_once("  ") else {
            continue;
        };
        if filename.starts_with("node-v") && filename.ends_with(&suffix) {
            return Ok(filename.to_string());
        }
    }

    bail!("failed to find a Linux archive for architecture `{arch}` in SHASUMS256.txt")
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

fn run_capture_command(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to start `{program}`"))?;
    if !output.status.success() {
        bail!(
            "command `{}` failed with status {}",
            format!("{program} {}", args.join(" ")),
            output.status
        );
    }

    String::from_utf8(output.stdout).context("command returned non-UTF-8 output")
}

fn home_dir() -> anyhow::Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")
}
