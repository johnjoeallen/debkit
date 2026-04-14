mod config;
mod install;
mod package;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "debkit", version, about = "DebKit CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Configure(ConfigureCommand),
    List,
    Package(PackageCommand),
    Install(InstallCommand),
    Uninstall(UninstallCommand),
    Status(StatusCommand),
}

#[derive(Debug, Args)]
struct PackageCommand {
    #[command(subcommand)]
    command: PackageSubcommand,
}

#[derive(Debug, Subcommand)]
enum PackageSubcommand {
    Deb(PackageDebArgs),
}

#[derive(Debug, Args)]
struct InstallCommand {
    #[command(subcommand)]
    command: InstallSubcommand,
}

#[derive(Debug, Args)]
struct ConfigureCommand {
    #[command(subcommand)]
    command: ConfigureSubcommand,
}

#[derive(Debug, Args)]
struct UninstallCommand {
    #[command(subcommand)]
    command: UninstallSubcommand,
}

#[derive(Debug, Subcommand)]
enum InstallSubcommand {
    Codex(InstallCodexArgs),
    Git,
    Npm(InstallNpmArgs),
    Ripgrep,
    Rust(InstallRustArgs),
    Variety,
    Foundation,
}

#[derive(Debug, Subcommand)]
enum ConfigureSubcommand {
    GitPrompt,
}

#[derive(Debug, Subcommand)]
enum UninstallSubcommand {
    Codex,
    Npm,
    Ripgrep,
}

#[derive(Debug, Args)]
struct StatusCommand {
    #[command(subcommand)]
    command: StatusSubcommand,
}

#[derive(Debug, Subcommand)]
enum StatusSubcommand {
    Variety,
}

#[derive(Debug, Args)]
struct InstallRustArgs {
    #[arg(long)]
    reinstall: bool,
}

#[derive(Debug, Args)]
struct InstallNpmArgs {
    #[arg(long, default_value = "latest")]
    version: String,
}

#[derive(Debug, Args)]
struct InstallCodexArgs {
    #[arg(long = "node-version", default_value = "latest")]
    node_version: String,
}

#[derive(Debug, Args)]
struct PackageDebArgs {
    #[arg(long, default_value_t = true)]
    release: bool,

    #[arg(long, default_value = "./dist")]
    output_dir: PathBuf,

    #[arg(long)]
    arch: Option<String>,

    #[arg(long)]
    verbose: bool,

    #[arg(long)]
    reinstall: bool,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Configure(configure) => match configure.command {
            ConfigureSubcommand::GitPrompt => {
                install::git_prompt::run()?;
            }
        },
        Commands::List => {
            install::list::run();
        }
        Commands::Package(pkg) => match pkg.command {
            PackageSubcommand::Deb(args) => {
                let output = package::deb::run(package::deb::Options {
                    release: args.release,
                    output_dir: args.output_dir,
                    arch: args.arch,
                    verbose: args.verbose,
                    reinstall: args.reinstall,
                })?;
                println!("{}", output.display());
            }
        },
        Commands::Install(install) => match install.command {
            InstallSubcommand::Codex(args) => {
                install::codex::run(args.node_version)?;
            }
            InstallSubcommand::Git => {
                install::git::run()?;
            }
            InstallSubcommand::Npm(args) => {
                install::npm::run(install::npm::Options {
                    version: args.version,
                })?;
            }
            InstallSubcommand::Ripgrep => {
                install::ripgrep::run()?;
            }
            InstallSubcommand::Rust(args) => {
                install::rust::run(install::rust::Options {
                    reinstall: args.reinstall,
                })?;
            }
            InstallSubcommand::Variety => {
                let config = config::load_or_init()?;
                install::variety::run(&config)?;
            }
            InstallSubcommand::Foundation => {
                let config = config::load_or_init()?;
                install::foundation::run(&config)?;
            }
        },
        Commands::Uninstall(uninstall) => match uninstall.command {
            UninstallSubcommand::Codex => {
                install::codex::uninstall()?;
            }
            UninstallSubcommand::Npm => {
                install::npm::uninstall()?;
            }
            UninstallSubcommand::Ripgrep => {
                install::ripgrep::uninstall()?;
            }
        },
        Commands::Status(status) => match status.command {
            StatusSubcommand::Variety => {
                let config = config::load_or_init()?;
                install::variety::print_status(&config)?;
            }
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_install_variety() {
        let cli = Cli::try_parse_from(["debkit", "install", "variety"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Variety
            })
        ));
    }

    #[test]
    fn parses_install_codex() {
        let cli = Cli::try_parse_from(["debkit", "install", "codex"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Codex(_)
            })
        ));
    }

    #[test]
    fn parses_install_git() {
        let cli = Cli::try_parse_from(["debkit", "install", "git"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Git
            })
        ));
    }

    #[test]
    fn parses_install_npm() {
        let cli = Cli::try_parse_from(["debkit", "install", "npm"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Npm(_)
            })
        ));
    }

    #[test]
    fn parses_install_npm_with_version() {
        let cli =
            Cli::try_parse_from(["debkit", "install", "npm", "--version", "24.12.0"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Npm(InstallNpmArgs { version })
            }) if version == "24.12.0"
        ));
    }

    #[test]
    fn parses_install_codex_with_node_version() {
        let cli = Cli::try_parse_from(["debkit", "install", "codex", "--node-version", "latest"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Codex(InstallCodexArgs { node_version })
            }) if node_version == "latest"
        ));
    }

    #[test]
    fn parses_install_ripgrep() {
        let cli = Cli::try_parse_from(["debkit", "install", "ripgrep"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Ripgrep
            })
        ));
    }

    #[test]
    fn parses_install_foundation() {
        let cli = Cli::try_parse_from(["debkit", "install", "foundation"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Foundation
            })
        ));
    }

    #[test]
    fn parses_configure_git_prompt() {
        let cli = Cli::try_parse_from(["debkit", "configure", "git-prompt"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Configure(ConfigureCommand {
                command: ConfigureSubcommand::GitPrompt
            })
        ));
    }

    #[test]
    fn parses_uninstall_codex() {
        let cli = Cli::try_parse_from(["debkit", "uninstall", "codex"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Uninstall(UninstallCommand {
                command: UninstallSubcommand::Codex
            })
        ));
    }

    #[test]
    fn parses_uninstall_npm() {
        let cli = Cli::try_parse_from(["debkit", "uninstall", "npm"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Uninstall(UninstallCommand {
                command: UninstallSubcommand::Npm
            })
        ));
    }

    #[test]
    fn parses_uninstall_ripgrep() {
        let cli = Cli::try_parse_from(["debkit", "uninstall", "ripgrep"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Uninstall(UninstallCommand {
                command: UninstallSubcommand::Ripgrep
            })
        ));
    }

    #[test]
    fn parses_status_variety() {
        let cli = Cli::try_parse_from(["debkit", "status", "variety"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Status(StatusCommand {
                command: StatusSubcommand::Variety
            })
        ));
    }
}
