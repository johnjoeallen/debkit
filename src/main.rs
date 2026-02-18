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
    List,
    Package(PackageCommand),
    Install(InstallCommand),
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

#[derive(Debug, Subcommand)]
enum InstallSubcommand {
    Rust(InstallRustArgs),
    Variety,
    Foundation,
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
