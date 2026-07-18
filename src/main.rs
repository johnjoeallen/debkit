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
    #[command(about = "Configure DebKit features")]
    Configure(ConfigureCommand),
    #[command(about = "Create or update the current host override config")]
    HostConfig,
    #[command(about = "List installable DebKit targets")]
    List,
    #[command(about = "Build DebKit packages")]
    Package(PackageCommand),
    #[command(about = "Install a DebKit target")]
    Install(InstallCommand),
    #[command(about = "Uninstall a DebKit target")]
    Uninstall(UninstallCommand),
    #[command(about = "Show status for a DebKit target")]
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
    Essentials,
    Git,
    Nis,
    NisClient,
    NisServer,
    Npm(InstallNpmArgs),
    Ripgrep,
    Rust(InstallRustArgs),
    SudoNopass,
    Variety,
    Foundation,
    WakeOnLan(InstallWakeOnLanArgs),
}

#[derive(Debug, Subcommand)]
enum ConfigureSubcommand {
    #[command(about = "Create or update ~/.config/debkit/hosts/<hostname>.toml")]
    HostConfig,
    #[command(about = "Configure the current user's Git-aware shell prompt")]
    GitPrompt,
    #[command(about = "Force-refresh NIS maps on a configured slave")]
    Nis(ConfigureNisArgs),
}

#[derive(Debug, Args)]
struct ConfigureNisArgs {
    #[command(subcommand)]
    command: Option<ConfigureNisSubcommand>,
}

#[derive(Debug, Subcommand)]
enum ConfigureNisSubcommand {
    #[command(about = "Add a slave FQDN to a master host config")]
    AddSlave(AddNisSlaveArgs),
}

#[derive(Debug, Args)]
struct AddNisSlaveArgs {
    slave: String,

    #[arg(long)]
    host: String,
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
    WakeOnLan,
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
struct InstallWakeOnLanArgs {
    #[arg(long)]
    dry_run: bool,
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
        Commands::HostConfig => {
            write_host_config()?;
        }
        Commands::Configure(configure) => match configure.command {
            ConfigureSubcommand::HostConfig => {
                write_host_config()?;
            }
            ConfigureSubcommand::GitPrompt => {
                install::git_prompt::run()?;
            }
            ConfigureSubcommand::Nis(args) => match args.command {
                Some(ConfigureNisSubcommand::AddSlave(add)) => {
                    let result = config::add_nis_slave_to_host(&add.host, &add.slave)?;
                    if result.added {
                        println!("Added {} to {} NIS slave list.", add.slave, add.host);
                    } else {
                        println!(
                            "{} is already present in {} NIS slave list.",
                            add.slave, add.host
                        );
                    }
                    println!("\nNext:");
                    println!(
                        "  1. Run `debkit configure nis` on {} to rebuild ypservers and push maps if enabled.",
                        add.host
                    );
                    println!(
                        "  2. Run `debkit install nis` on {} if it has not already been configured as a slave.",
                        add.slave
                    );
                    println!(
                        "  3. Run `debkit configure nis` on {} to force-refresh stale slave maps.",
                        add.slave
                    );
                    println!("\nUpdated: {}", result.path.display());
                }
                None => {
                    let config = config::load_or_init()?;
                    install::nis::configure(&config.nis)?;
                }
            },
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
            InstallSubcommand::Essentials => {
                let config = config::load_or_init()?;
                install::essentials::run(&config.essentials)?;
            }
            InstallSubcommand::Git => {
                install::git::run()?;
            }
            InstallSubcommand::Nis => {
                let config = config::load_or_init()?;
                install::nis::run(install::nis::Role::Configured, &config.nis)?;
            }
            InstallSubcommand::NisClient => {
                let config = config::load_or_init()?;
                install::nis::run(install::nis::Role::Client, &config.nis)?;
            }
            InstallSubcommand::NisServer => {
                let config = config::load_or_init()?;
                install::nis::run(install::nis::Role::Server, &config.nis)?;
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
            InstallSubcommand::SudoNopass => {
                let config = config::load_or_init()?;
                install::sudo_nopass::run(&config.sudo_nopass)?;
                install::nis::rebuild_and_push_maps(&config.nis)?;
            }
            InstallSubcommand::Variety => {
                let config = config::load_or_init()?;
                install::variety::run(&config)?;
            }
            InstallSubcommand::Foundation => {
                let config = config::load_or_init()?;
                install::foundation::run(&config)?;
            }
            InstallSubcommand::WakeOnLan(args) => {
                let config = config::load_or_init()?;
                if args.dry_run {
                    install::wake_on_lan::dry_run(&config)?;
                } else {
                    install::wake_on_lan::run(&config)?;
                }
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
            StatusSubcommand::WakeOnLan => {
                let config = config::load_or_init()?;
                install::wake_on_lan::print_status(&config)?;
            }
        },
    }

    Ok(())
}

fn write_host_config() -> anyhow::Result<()> {
    let path = config::configure_complete_for_current_host()?;
    println!("Wrote host override config: {}", path.display());
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
    fn parses_install_essentials() {
        let cli = Cli::try_parse_from(["debkit", "install", "essentials"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Essentials
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
    fn parses_install_nis() {
        let cli = Cli::try_parse_from(["debkit", "install", "nis"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::Nis
            })
        ));
    }

    #[test]
    fn parses_install_nis_client() {
        let cli = Cli::try_parse_from(["debkit", "install", "nis-client"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::NisClient
            })
        ));
    }

    #[test]
    fn parses_install_nis_server() {
        let cli = Cli::try_parse_from(["debkit", "install", "nis-server"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::NisServer
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
    fn parses_install_sudo_nopass() {
        let cli = Cli::try_parse_from(["debkit", "install", "sudo-nopass"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::SudoNopass
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
    fn parses_install_wake_on_lan() {
        let cli = Cli::try_parse_from(["debkit", "install", "wake-on-lan"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::WakeOnLan(_)
            })
        ));
    }

    #[test]
    fn parses_install_wake_on_lan_dry_run() {
        let cli = Cli::try_parse_from(["debkit", "install", "wake-on-lan", "--dry-run"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Install(InstallCommand {
                command: InstallSubcommand::WakeOnLan(InstallWakeOnLanArgs { dry_run: true })
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
    fn parses_configure_nis() {
        let cli = Cli::try_parse_from(["debkit", "configure", "nis"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Configure(ConfigureCommand {
                command: ConfigureSubcommand::Nis(ConfigureNisArgs { command: None })
            })
        ));
    }

    #[test]
    fn parses_configure_nis_add_slave() {
        let cli = Cli::try_parse_from([
            "debkit",
            "configure",
            "nis",
            "add-slave",
            "--host",
            "iris",
            "spitfire.dublinux.lan",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Configure(ConfigureCommand {
                command: ConfigureSubcommand::Nis(ConfigureNisArgs {
                    command: Some(ConfigureNisSubcommand::AddSlave(AddNisSlaveArgs { .. }))
                })
            })
        ));
    }

    #[test]
    fn parses_configure_host_config() {
        let cli = Cli::try_parse_from(["debkit", "configure", "host-config"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Configure(ConfigureCommand {
                command: ConfigureSubcommand::HostConfig
            })
        ));
    }

    #[test]
    fn parses_top_level_host_config() {
        let cli = Cli::try_parse_from(["debkit", "host-config"]).unwrap();
        assert!(matches!(cli.command, Commands::HostConfig));
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

    #[test]
    fn parses_status_wake_on_lan() {
        let cli = Cli::try_parse_from(["debkit", "status", "wake-on-lan"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Status(StatusCommand {
                command: StatusSubcommand::WakeOnLan
            })
        ));
    }
}
