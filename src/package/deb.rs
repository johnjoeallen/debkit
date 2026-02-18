use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

#[derive(Debug, Clone)]
pub struct Options {
    pub release: bool,
    pub output_dir: PathBuf,
    pub arch: Option<String>,
    pub verbose: bool,
    pub reinstall: bool,
}

pub fn run(options: Options) -> anyhow::Result<PathBuf> {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));

    ensure_cargo_deb_available(options.reinstall, options.verbose)?;

    let mut args = vec!["deb".to_string()];
    if let Some(arch) = &options.arch {
        args.push("--deb-arch".to_string());
        args.push(arch.clone());
    }
    if !options.release {
        args.push("--profile".to_string());
        args.push("dev".to_string());
    }

    run_command("cargo", &args, project_root, options.verbose)
        .with_context(|| "failed to run cargo-deb package build")?;

    let debian_dir = project_root.join("target").join("debian");
    let newest = newest_matching_deb(&debian_dir, "debkit_")?;

    fs::create_dir_all(&options.output_dir).with_context(|| {
        format!(
            "failed to create output directory {}",
            options.output_dir.display()
        )
    })?;

    let filename = newest
        .file_name()
        .context("newest .deb path does not include a filename")?;
    let output_path = options.output_dir.join(filename);

    if options.verbose {
        eprintln!("copy {} -> {}", newest.display(), output_path.display());
    }

    fs::copy(&newest, &output_path).with_context(|| {
        format!(
            "failed to copy artifact from {} to {}",
            newest.display(),
            output_path.display()
        )
    })?;

    Ok(absolute_path(&output_path)?)
}

fn ensure_cargo_deb_available(reinstall: bool, verbose: bool) -> anyhow::Result<()> {
    if reinstall {
        let install_args = vec![
            "install".to_string(),
            "--locked".to_string(),
            "--force".to_string(),
            "cargo-deb".to_string(),
        ];
        run_command(
            "cargo",
            &install_args,
            Path::new(env!("CARGO_MANIFEST_DIR")),
            verbose,
        )
        .with_context(|| "failed to reinstall cargo-deb")?;
        return Ok(());
    }

    let output = Command::new("cargo")
        .args(["deb", "--version"])
        .output()
        .context("`cargo` executable was not found in PATH")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if verbose {
        eprintln!("cargo deb --version stdout: {stdout}");
        eprintln!("cargo deb --version stderr: {stderr}");
    }

    bail!(
        "cargo-deb is required but not installed. Install it with: cargo install --locked cargo-deb"
    );
}

fn run_command(program: &str, args: &[String], cwd: &Path, verbose: bool) -> anyhow::Result<()> {
    if verbose {
        eprintln!(
            "run (cwd: {}): {} {}",
            cwd.display(),
            program,
            args.join(" ")
        );
    }

    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to start `{program}`"))?;

    if output.status.success() {
        if verbose {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stdout.trim().is_empty() {
                eprintln!("stdout:\n{stdout}");
            }
            if !stderr.trim().is_empty() {
                eprintln!("stderr:\n{stderr}");
            }
        }
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "command `{}` failed with status {}\nstdout:\n{}\nstderr:\n{}",
        format!("{} {}", program, args.join(" ")),
        output.status,
        stdout,
        stderr,
    );
}

pub fn newest_matching_deb(dir: &Path, prefix: &str) -> anyhow::Result<PathBuf> {
    let mut candidates = Vec::new();

    let entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read artifact directory {}", dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let filename = match path.file_name().and_then(OsStr::to_str) {
            Some(name) => name,
            None => continue,
        };
        if !filename.starts_with(prefix) {
            continue;
        }
        if path.extension().and_then(OsStr::to_str) != Some("deb") {
            continue;
        }

        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .with_context(|| format!("failed to read mtime for {}", path.display()))?;
        candidates.push((modified, path));
    }

    candidates.sort_by(|(a_time, a_path), (b_time, b_path)| {
        a_time
            .cmp(b_time)
            .then_with(|| a_path.file_name().cmp(&b_path.file_name()))
    });

    candidates.pop().map(|(_, path)| path).with_context(|| {
        format!(
            "no .deb artifacts found in {} matching prefix {}",
            dir.display(),
            prefix
        )
    })
}

fn absolute_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(std::env::current_dir()
        .context("failed to read current working directory")?
        .join(path))
}

#[cfg(test)]
mod tests {
    use super::newest_matching_deb;
    use std::fs;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    fn temp_test_dir() -> PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("debkit-test-{pid}-{nanos}"))
    }

    #[test]
    fn picks_newest_matching_deb_file() {
        let dir = temp_test_dir();
        fs::create_dir_all(&dir).expect("create temp test dir");

        let first = dir.join("debkit_0.1.0_amd64.deb");
        fs::write(&first, b"a").expect("write first");

        thread::sleep(Duration::from_millis(1100));

        let second = dir.join("debkit_0.2.0_amd64.deb");
        fs::write(&second, b"b").expect("write second");

        let selected = newest_matching_deb(&dir, "debkit_").expect("pick newest");
        assert_eq!(selected, second);

        fs::remove_dir_all(&dir).expect("cleanup temp test dir");
    }
}
