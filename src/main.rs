mod config;
mod engine;
mod install;
mod modules;
mod package;

use std::path::PathBuf;

use anyhow::{Context as _, bail};
use clap::{Args, Parser, Subcommand};

use engine::module::{Context as ModuleContext, Module};

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
    #[command(about = "Discover observed state for a module (or every module)")]
    Inspect(ModuleArgs),
    #[command(about = "Discover and diagnose a module's state against declared intent")]
    Diagnose(ModuleArgs),
    #[command(about = "Show the change plan a module would apply")]
    Plan(ModuleArgs),
    #[command(about = "Apply a module's change plan and verify the result")]
    Apply(ApplyArgs),
    #[command(about = "Run a module's functional verification checks")]
    Verify(ModuleArgs),
    #[command(about = "Reverse a module's most recent applied change plan")]
    Rollback(RollbackArgs),
    #[command(about = "Show the most recently recorded evidence for a module (or every module)")]
    History(ModuleArgs),
}

#[derive(Debug, Args)]
struct ModuleArgs {
    /// Dotted module name, e.g. `core.inspect`. Omits to run every registered module.
    module: Option<String>,

    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ApplyArgs {
    /// Dotted module name, e.g. `core.inspect`. Omits to run every registered module.
    module: Option<String>,

    #[arg(long)]
    json: bool,

    /// Materialize the change plan into /tmp/debkit/<run-id>/ instead of touching
    /// real files. Nothing is applied, nothing is verified, no evidence is written —
    /// safe to run without root even for changes that would need it to actually apply.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct RollbackArgs {
    /// Dotted module name whose most recent applied plan should be reversed.
    module: String,

    /// Roll back a specific journal file instead of the most recent one.
    #[arg(long)]
    journal: Option<PathBuf>,
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
    command: Option<StatusSubcommand>,

    /// Dotted module name for the new lifecycle-based modules, e.g. `core.inspect`.
    /// Ignored when a legacy subcommand above matched.
    module: Option<String>,
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
            run_list_modules();
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
            Some(StatusSubcommand::Variety) => {
                let config = config::load_or_init()?;
                install::variety::print_status(&config)?;
            }
            Some(StatusSubcommand::WakeOnLan) => {
                let config = config::load_or_init()?;
                install::wake_on_lan::print_status(&config)?;
            }
            None => match status.module {
                Some(module) => run_status(&module)?,
                None => {
                    bail!(
                        "specify a target: `debkit status variety`, `debkit status wake-on-lan`, or a module name like `debkit status core.inspect`"
                    );
                }
            },
        },
        Commands::Inspect(args) => run_inspect(&args)?,
        Commands::Diagnose(args) => run_diagnose(&args)?,
        Commands::Plan(args) => run_plan(&args)?,
        Commands::Apply(args) => run_apply(&args)?,
        Commands::Verify(args) => run_verify(&args)?,
        Commands::Rollback(args) => run_rollback(&args)?,
        Commands::History(args) => run_history(&args)?,
    }

    Ok(())
}

fn resolve_modules(selector: &Option<String>) -> anyhow::Result<Vec<Box<dyn Module>>> {
    match selector {
        Some(name) => {
            let module = modules::find(name)
                .with_context(|| format!("unknown module `{name}` (see `debkit list`)"))?;
            Ok(vec![module])
        }
        None => Ok(modules::registry()),
    }
}

/// Lists every module registered for the `inspect`/`diagnose`/`plan`/`apply`/`verify`
/// lifecycle, alongside the legacy install targets `install::list::run()` prints —
/// these modules aren't installable targets, so they didn't previously show up
/// anywhere in `debkit list`'s output.
fn run_list_modules() {
    let mut modules = modules::registry();
    modules.sort_by_key(|module| module.name());
    println!("\nLifecycle modules (debkit inspect|diagnose|plan|apply|verify <name>):");
    let width = modules
        .iter()
        .map(|module| module.name().len())
        .max()
        .unwrap_or(0);
    for module in &modules {
        println!(
            "  {:width$}  {}",
            module.name(),
            module.description(),
            width = width
        );
    }
}

fn run_inspect(args: &ModuleArgs) -> anyhow::Result<()> {
    let config = config::load_or_init()?;
    let ctx = ModuleContext {
        hostname: config.host.name.clone(),
        config: &config,
    };
    for module in resolve_modules(&args.module)? {
        let observation = module.discover(&ctx)?;
        if args.json {
            let rendered = serde_json::json!({
                "module": module.name(),
                "host": ctx.hostname,
                "owner": observation.owner,
                "observed": observation.data,
                "warnings": observation.warnings,
            });
            println!("{}", serde_json::to_string_pretty(&rendered)?);
        } else {
            println!("== {} ==", module.name());
            println!("owner: {}", observation.owner.as_deref().unwrap_or("none"));
            println!("{}", serde_json::to_string_pretty(&observation.data)?);
            for warning in &observation.warnings {
                println!("warning: {warning}");
            }
        }
    }
    Ok(())
}

fn run_diagnose(args: &ModuleArgs) -> anyhow::Result<()> {
    let config = config::load_or_init()?;
    let ctx = ModuleContext {
        hostname: config.host.name.clone(),
        config: &config,
    };
    for module in resolve_modules(&args.module)? {
        let observation = module.discover(&ctx)?;
        let diagnosis = module.diagnose(&ctx, &observation);
        println!("== {} ==", module.name());
        println!(
            "compliant: {}",
            if diagnosis.compliant { "yes" } else { "no" }
        );
        if let Some(conflict) = &diagnosis.conflict {
            println!("conflict: {}", conflict.join(", "));
        }
        for finding in &diagnosis.findings {
            println!("- {finding}");
        }
    }
    Ok(())
}

/// `debkit status <module>` — the `Module`-based counterpart to the legacy
/// `status variety`/`status wake-on-lan` subcommands: discover()+diagnose(), rendered
/// for humans, on a single named module.
fn run_status(module_name: &str) -> anyhow::Result<()> {
    let config = config::load_or_init()?;
    let ctx = ModuleContext {
        hostname: config.host.name.clone(),
        config: &config,
    };
    let module = modules::find(module_name)
        .with_context(|| format!("unknown module `{module_name}` (see `debkit list`)"))?;

    let observation = module.discover(&ctx)?;
    let diagnosis = module.diagnose(&ctx, &observation);

    println!("== {} ==", module.name());
    println!("owner: {}", observation.owner.as_deref().unwrap_or("none"));
    println!(
        "compliant: {}",
        if diagnosis.compliant { "yes" } else { "no" }
    );
    if let Some(conflict) = &diagnosis.conflict {
        println!("conflict: {}", conflict.join(", "));
    }
    for finding in &diagnosis.findings {
        println!("- {finding}");
    }
    for warning in &observation.warnings {
        println!("warning: {warning}");
    }
    Ok(())
}

fn run_plan(args: &ModuleArgs) -> anyhow::Result<()> {
    let config = config::load_or_init()?;
    let ctx = ModuleContext {
        hostname: config.host.name.clone(),
        config: &config,
    };
    for module in resolve_modules(&args.module)? {
        let observation = module.discover(&ctx)?;
        let diagnosis = module.diagnose(&ctx, &observation);
        println!("== {} ==", module.name());
        if let Some(conflict) = &diagnosis.conflict {
            println!(
                "refusing to plan: competing owners are active ({}); set an explicit owner override first",
                conflict.join(", ")
            );
            continue;
        }
        let plan = module.plan(&ctx, &observation, &diagnosis)?;
        print!("{}", plan.render_dry_run());
    }
    Ok(())
}

fn run_apply(args: &ApplyArgs) -> anyhow::Result<()> {
    let config = config::load_or_init()?;
    let ctx = ModuleContext {
        hostname: config.host.name.clone(),
        config: &config,
    };
    for module in resolve_modules(&args.module)? {
        let observation = module.discover(&ctx)?;
        let diagnosis = module.diagnose(&ctx, &observation);
        println!("== {} ==", module.name());
        if let Some(conflict) = &diagnosis.conflict {
            println!(
                "refusing to apply: competing owners are active ({}); set an explicit owner override first",
                conflict.join(", ")
            );
            continue;
        }
        let plan = module.plan(&ctx, &observation, &diagnosis)?;
        if plan.is_empty() {
            println!(
                "already compliant; no changes {}",
                if args.dry_run { "to stage" } else { "applied" }
            );
            if args.dry_run {
                continue;
            }
            let evidence = engine::evidence::Evidence {
                module: module.name().to_string(),
                host: ctx.hostname.clone(),
                status: engine::evidence::Status::Compliant,
                owner: observation.owner.clone(),
                observed: observation.data.clone(),
                changes: Vec::new(),
                verification: Vec::new(),
                artifacts: Vec::new(),
            };
            let path = engine::evidence::write_evidence(&evidence)?;
            println!("evidence: {}", path.display());
            continue;
        }

        if args.dry_run {
            let staged = engine::apply::stage_plan(&plan)?;
            println!(
                "staged {} file change(s) under {} (run {})",
                staged.files.len(),
                staged.root.display(),
                staged.run_id
            );
            for file in &staged.files {
                println!(
                    "  {} -> {}",
                    file.target.display(),
                    file.staged_path.display()
                );
                match &file.pristine_path {
                    Some(pristine) => println!("    pristine copy: {}", pristine.display()),
                    None => {
                        println!("    pristine copy: none (file didn't exist or wasn't readable)")
                    }
                }
            }
            if !staged.skipped_actions.is_empty() {
                println!("not executed in dry-run mode (no file content to preview):");
                for action in &staged.skipped_actions {
                    println!("  {action}");
                }
            }
            continue;
        }

        let changes_summary: Vec<engine::evidence::AppliedChangeSummary> = (&plan).into();
        let applied = engine::apply::apply_plan(module.name(), &ctx.hostname, &plan)?;
        let journal_path = engine::apply::write_journal(&applied)?;
        println!(
            "applied {} change(s); journal: {}",
            applied.entries.len(),
            journal_path.display()
        );

        let verification = module.verify(&ctx)?;
        let failed: Vec<&engine::evidence::VerificationResult> = verification
            .iter()
            .filter(|check| check.result == engine::evidence::CheckResult::Fail)
            .collect();

        let status = if failed.is_empty() {
            engine::evidence::Status::Changed
        } else {
            println!("verification failed; rolling back");
            engine::apply::rollback(&applied)?;
            engine::evidence::Status::Error
        };

        let evidence = engine::evidence::Evidence {
            module: module.name().to_string(),
            host: ctx.hostname.clone(),
            status,
            owner: observation.owner.clone(),
            observed: observation.data.clone(),
            changes: changes_summary,
            verification,
            artifacts: vec![journal_path.display().to_string()],
        };
        let path = engine::evidence::write_evidence(&evidence)?;
        println!("evidence: {}", path.display());
    }
    Ok(())
}

fn run_verify(args: &ModuleArgs) -> anyhow::Result<()> {
    let config = config::load_or_init()?;
    let ctx = ModuleContext {
        hostname: config.host.name.clone(),
        config: &config,
    };
    for module in resolve_modules(&args.module)? {
        println!("== {} ==", module.name());
        for check in module.verify(&ctx)? {
            println!(
                "[{:?}] {}{}",
                check.result,
                check.check,
                check
                    .detail
                    .as_deref()
                    .map(|detail| format!(" — {detail}"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(())
}

/// Reads back the evidence `apply`/verify-on-apply already write to
/// `/var/lib/debkit/<module>/<hostname>.json` — previously write-only from the CLI's
/// perspective; nothing surfaced it until this command.
fn run_history(args: &ModuleArgs) -> anyhow::Result<()> {
    let config = config::load_or_init()?;
    let ctx = ModuleContext {
        hostname: config.host.name.clone(),
        config: &config,
    };
    for module in resolve_modules(&args.module)? {
        let evidence = engine::evidence::read_evidence(module.name(), &ctx.hostname)?;
        match evidence {
            Some(evidence) if args.json => {
                println!("{}", serde_json::to_string_pretty(&evidence)?);
            }
            Some(evidence) => {
                println!("== {} ==", module.name());
                println!("status: {:?}", evidence.status);
                if let Some(owner) = &evidence.owner {
                    println!("owner: {owner}");
                }
                if !evidence.changes.is_empty() {
                    println!("changes applied:");
                    for change in &evidence.changes {
                        println!("  [{}] {}", change.risk.as_str(), change.description);
                    }
                }
                if !evidence.verification.is_empty() {
                    println!("verification:");
                    for check in &evidence.verification {
                        println!(
                            "  [{:?}] {}{}",
                            check.result,
                            check.check,
                            check
                                .detail
                                .as_deref()
                                .map(|detail| format!(" — {detail}"))
                                .unwrap_or_default()
                        );
                    }
                }
                if !evidence.artifacts.is_empty() {
                    println!("artifacts: {}", evidence.artifacts.join(", "));
                }
            }
            None => {
                println!("== {} ==", module.name());
                println!(
                    "no evidence recorded (run `debkit apply {}` first)",
                    module.name()
                );
            }
        }
    }
    Ok(())
}

fn run_rollback(args: &RollbackArgs) -> anyhow::Result<()> {
    let config = config::load_or_init()?;
    let hostname = config.host.name.clone();

    let journal_path = match &args.journal {
        Some(path) => path.clone(),
        None => engine::apply::find_latest_journal(&args.module, &hostname)?.with_context(|| {
            format!(
                "no rollback journal found for `{}` on `{hostname}`; pass --journal <path> if it was written elsewhere",
                args.module
            )
        })?,
    };

    let applied = engine::apply::read_journal(&journal_path)?;
    engine::apply::rollback(&applied)?;
    println!(
        "rolled back {} change(s) from {}",
        applied.entries.len(),
        journal_path.display()
    );
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
                command: Some(StatusSubcommand::Variety),
                ..
            })
        ));
    }

    #[test]
    fn parses_status_wake_on_lan() {
        let cli = Cli::try_parse_from(["debkit", "status", "wake-on-lan"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Status(StatusCommand {
                command: Some(StatusSubcommand::WakeOnLan),
                ..
            })
        ));
    }

    #[test]
    fn parses_status_module_name() {
        let cli = Cli::try_parse_from(["debkit", "status", "core.inspect"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Status(StatusCommand {
                command: None,
                module: Some(module),
            }) if module == "core.inspect"
        ));
    }

    #[test]
    fn parses_inspect_with_module_and_json() {
        let cli = Cli::try_parse_from(["debkit", "inspect", "core.inspect", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Inspect(ModuleArgs { module: Some(m), json: true }) if m == "core.inspect"
        ));
    }

    #[test]
    fn parses_inspect_without_module_selector() {
        let cli = Cli::try_parse_from(["debkit", "inspect"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Inspect(ModuleArgs {
                module: None,
                json: false
            })
        ));
    }

    #[test]
    fn parses_diagnose_plan_apply_verify() {
        assert!(matches!(
            Cli::try_parse_from(["debkit", "diagnose", "core.inspect"])
                .unwrap()
                .command,
            Commands::Diagnose(ModuleArgs {
                module: Some(_),
                ..
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["debkit", "plan", "core.inspect"])
                .unwrap()
                .command,
            Commands::Plan(ModuleArgs {
                module: Some(_),
                ..
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["debkit", "apply", "core.inspect"])
                .unwrap()
                .command,
            Commands::Apply(ApplyArgs {
                module: Some(_),
                dry_run: false,
                ..
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["debkit", "apply", "hardware.reboot", "--dry-run"])
                .unwrap()
                .command,
            Commands::Apply(ApplyArgs {
                module: Some(_),
                dry_run: true,
                ..
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["debkit", "verify", "core.inspect"])
                .unwrap()
                .command,
            Commands::Verify(ModuleArgs {
                module: Some(_),
                ..
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["debkit", "history", "core.inspect"])
                .unwrap()
                .command,
            Commands::History(ModuleArgs {
                module: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn parses_history_without_module_selector() {
        let cli = Cli::try_parse_from(["debkit", "history"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::History(ModuleArgs {
                module: None,
                json: false
            })
        ));
    }

    #[test]
    fn parses_rollback_with_and_without_journal() {
        let cli = Cli::try_parse_from(["debkit", "rollback", "identity.nis"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Rollback(RollbackArgs { module, journal: None }) if module == "identity.nis"
        ));

        let cli = Cli::try_parse_from([
            "debkit",
            "rollback",
            "identity.nis",
            "--journal",
            "/var/lib/debkit/journal/spitfire-identity-nis-100.json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Rollback(RollbackArgs {
                journal: Some(_),
                ..
            })
        ));
    }
}
