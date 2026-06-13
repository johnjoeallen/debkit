use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, bail};

use crate::config::NisConfig;

const DEFAULTDOMAIN_PATH: &str = "/etc/defaultdomain";
const YP_CONF_PATH: &str = "/etc/yp.conf";
const NSSWITCH_PATH: &str = "/etc/nsswitch.conf";
const YP_MAP_ROOT: &str = "/var/yp";
const YPSERVERS_SOURCE_PATH: &str = "/var/yp/ypservers";
const YPINIT_PATH: &str = "/usr/lib/yp/ypinit";
const YPXFR_PATH: &str = "/usr/lib/yp/ypxfr";
const MAKEDBM_PATH: &str = "/usr/lib/yp/makedbm";
const YPPUSH_PATH: &str = "/usr/sbin/yppush";
const YPXFR_REFRESH_SCRIPTS: &[&str] = &[
    "/usr/lib/yp/ypxfr_1perhour",
    "/usr/lib/yp/ypxfr_2perday",
    "/usr/lib/yp/ypxfr_1perday",
];
const FALLBACK_MAPS: &[&str] = &[
    "rpc.bynumber",
    "hosts.byname",
    "netid.byname",
    "hosts.byaddr",
    "netgroup.byuser",
    "group.bygid",
    "netgroup.byhost",
    "services.byname",
    "rpc.byname",
    "passwd.byname",
    "ypservers",
    "shadow.byname",
    "passwd.byuid",
    "services.byservicename",
    "protocols.byname",
    "netgroup",
    "protocols.bynumber",
    "group.byname",
];

#[derive(Debug, Clone, Copy)]
pub enum Role {
    Configured,
    Client,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NisRole {
    Master,
    Slave,
    Client,
    ServerOnly,
}

#[derive(Debug, Clone)]
struct NisPlan {
    role: NisRole,
    domain: String,
    master: Option<String>,
    admin_user: String,
    client_servers: Vec<String>,
    slaves: Vec<String>,
    push_to_slaves: bool,
    force_refresh_maps: bool,
    local_admin_groups: Vec<String>,
    packages: Vec<&'static str>,
    services: Vec<&'static str>,
    optional_services: Vec<&'static str>,
}

pub fn run(role: Role, config: &NisConfig) -> anyhow::Result<()> {
    if !config.enabled {
        println!("NIS configuration is disabled (`nis.enabled = false`).");
        return Ok(());
    }

    let plan = build_plan(role, config)?;
    install_packages(&plan.packages, plan.role.label())?;
    warn_if_no_local_admin(&plan.local_admin_groups);

    ensure_root_file(Path::new(DEFAULTDOMAIN_PATH), &format!("{}\n", plan.domain))?;
    set_runtime_nis_domain(&plan.domain)?;
    if plan.role.includes_client() {
        ensure_nsswitch_uses_files_then_nis(Path::new(NSSWITCH_PATH))?;
    }

    match plan.role {
        NisRole::Master => configure_master(&plan)?,
        NisRole::Slave => configure_slave(&plan)?,
        NisRole::Client => configure_client(&plan)?,
        NisRole::ServerOnly => configure_server_only(&plan)?,
    }
    if plan.role.includes_client() {
        validate_nis_admin_group_membership(&plan);
    }

    println!("NIS configured:");
    println!("  Role: {}", plan.role.config_value());
    println!("  Domain: {}", plan.domain);
    if let Some(master) = &plan.master {
        println!("  Master: {master}");
    }
    if plan.role.includes_client() {
        println!("  NSS: local files are kept before NIS (`files nis`).");
    }
    println!(
        "  Services: {}",
        if plan.services.is_empty() {
            "none".to_string()
        } else {
            plan.services.join(", ")
        }
    );
    println!(
        "Keep a local sudo-capable account on every machine for recovery; DebKit does not remove or rewrite local account databases."
    );

    Ok(())
}

pub fn rebuild_and_push_maps(config: &NisConfig) -> anyhow::Result<()> {
    if !config.enabled || config.role.trim() != "master" {
        return Ok(());
    }
    let domain = config.domain.trim();
    if domain.is_empty() {
        return Ok(());
    }
    println!("Rebuilding NIS maps after group changes...");
    rebuild_master_maps(domain)?;
    let plan = build_plan(Role::Configured, config)?;
    push_maps_to_slaves_if_requested(&plan)?;
    Ok(())
}

pub fn configure(config: &NisConfig) -> anyhow::Result<()> {
    if !config.enabled {
        bail!("NIS configuration is disabled (`nis.enabled = false`)");
    }
    let plan = build_plan(Role::Configured, config)?;
    if plan.role == NisRole::Master {
        configure_master(&plan)?;
        println!("NIS master maps rebuilt.");
        return Ok(());
    }
    if plan.role != NisRole::Slave {
        bail!("`debkit configure nis` requires `nis.role = \"master\"` or `nis.role = \"slave\"`");
    }
    let master = plan
        .master
        .as_deref()
        .context("slave plan missing master")?;

    ensure_root_file(Path::new(DEFAULTDOMAIN_PATH), &format!("{}\n", plan.domain))?;
    set_runtime_nis_domain(&plan.domain)?;
    ensure_nsswitch_uses_files_then_nis(Path::new(NSSWITCH_PATH))?;
    ensure_root_file(
        Path::new(YP_CONF_PATH),
        &render_yp_conf(&plan.domain, &[master]),
    )?;
    enable_and_start_service("rpcbind")?;
    enable_and_start_service("ypbind")?;
    enable_and_start_service("ypserv")?;
    enable_optional_services(&plan)?;

    force_refresh_slave_maps(&plan.domain, master)?;

    if Path::new(YP_MAP_ROOT).join(&plan.domain).exists() {
        ensure_root_file(
            Path::new(YP_CONF_PATH),
            &render_yp_conf(&plan.domain, &plan.client_servers_as_strs()),
        )?;
        restart_service("ypbind")?;
    }

    register_slave_with_master(&plan);

    println!("NIS slave maps force-refreshed from {master}.");
    Ok(())
}

fn configure_master(plan: &NisPlan) -> anyhow::Result<()> {
    ensure_root_file(
        Path::new(YP_CONF_PATH),
        &render_yp_conf(&plan.domain, &["127.0.0.1"]),
    )?;
    enable_required_services(plan)?;

    let master = current_fqdn(&plan.domain)?;
    initialize_master_maps_if_needed(&plan.domain)?;
    ensure_ypservers_source(&master, &plan.slaves)?;
    rebuild_ypservers_map(&plan.domain)?;
    validate_ypservers_map(&plan.domain, &master, &plan.slaves)?;
    rebuild_master_maps(&plan.domain)?;
    push_maps_to_slaves_if_requested(plan)?;

    Ok(())
}

fn configure_slave(plan: &NisPlan) -> anyhow::Result<()> {
    let master = plan
        .master
        .as_deref()
        .context("slave plan missing master")?;
    ensure_root_file(
        Path::new(YP_CONF_PATH),
        &render_yp_conf(&plan.domain, &[master]),
    )?;

    enable_and_start_service("rpcbind")?;
    enable_and_start_service("ypbind")?;
    enable_and_start_service("ypserv")?;
    enable_optional_services(plan)?;

    if plan.force_refresh_maps {
        force_refresh_slave_maps(&plan.domain, master)?;
    } else {
        initialize_or_refresh_slave_maps(&plan.domain, master)?;
    }

    if Path::new(YP_MAP_ROOT).join(&plan.domain).exists() {
        ensure_root_file(
            Path::new(YP_CONF_PATH),
            &render_yp_conf(&plan.domain, &plan.client_servers_as_strs()),
        )?;
        restart_service("ypbind")?;
    }

    register_slave_with_master(plan);

    Ok(())
}

fn register_slave_with_master(plan: &NisPlan) {
    let master = match plan.master.as_deref() {
        Some(m) => m,
        None => return,
    };

    let slave_fqdn = match current_fqdn(&plan.domain) {
        Ok(fqdn) if !fqdn.is_empty() => fqdn,
        _ => {
            eprintln!(
                "warning: could not determine local FQDN; skipping registration with master {master}."
            );
            eprintln!("  To register manually, run on {master}:");
            eprintln!("    debkit configure nis add-slave --host \"$(hostname)\" <this-host-fqdn>");
            eprintln!("    sudo debkit configure nis");
            return;
        }
    };

    let ssh_target = if plan.admin_user.is_empty() {
        master.to_string()
    } else {
        format!("{}@{}", plan.admin_user, master)
    };

    let remote_cmd = format!(
        "debkit configure nis add-slave --host \"$(hostname)\" {slave_fqdn} && sudo debkit configure nis"
    );

    println!("Registering slave {slave_fqdn} with NIS master {master}...");

    let status = Command::new("ssh")
        .args([&ssh_target, &remote_cmd])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("Registered with master {master}.");
        }
        Ok(s) => {
            eprintln!(
                "warning: SSH to {master} exited with status {}; slave was not registered.",
                s.code()
                    .map_or_else(|| "unknown".to_string(), |c| c.to_string())
            );
            eprintln!("  To register manually, run on {master}:");
            eprintln!("    debkit configure nis add-slave --host \"$(hostname)\" {slave_fqdn}");
            eprintln!("    sudo debkit configure nis");
        }
        Err(e) => {
            eprintln!("warning: could not SSH to {master}: {e}; slave was not registered.");
            eprintln!("  To register manually, run on {master}:");
            eprintln!("    debkit configure nis add-slave --host \"$(hostname)\" {slave_fqdn}");
            eprintln!("    sudo debkit configure nis");
        }
    }
}

fn configure_client(plan: &NisPlan) -> anyhow::Result<()> {
    ensure_root_file(
        Path::new(YP_CONF_PATH),
        &render_yp_conf(&plan.domain, &plan.client_servers_as_strs()),
    )?;
    enable_required_services(plan)?;
    Ok(())
}

fn configure_server_only(plan: &NisPlan) -> anyhow::Result<()> {
    enable_required_services(plan)?;
    enable_optional_services(plan)?;
    println!(
        "warning: `nis-server` is a compatibility install target only; configure role `master` or `slave` for normal DebKit NIS use."
    );
    Ok(())
}

fn build_plan(requested: Role, config: &NisConfig) -> anyhow::Result<NisPlan> {
    let role = match requested {
        Role::Configured => parse_role(&config.role)?,
        Role::Client => NisRole::Client,
        Role::Server => NisRole::ServerOnly,
    };
    let domain = config.domain.trim().to_string();
    if domain.is_empty() {
        bail!("`nis.domain` must be set when `nis.enabled = true`");
    }

    let mut master = None;
    let client_servers = match role {
        NisRole::Master => vec!["127.0.0.1".to_string()],
        NisRole::Slave => {
            let configured_master = config.master.trim();
            if configured_master.is_empty() {
                bail!("`nis.master` must be set when `nis.role = \"slave\"`");
            }
            master = Some(configured_master.to_string());
            if config.prefer_local {
                vec!["127.0.0.1".to_string(), configured_master.to_string()]
            } else {
                vec![configured_master.to_string()]
            }
        }
        NisRole::Client => {
            let mut servers = Vec::new();
            if !config.server.trim().is_empty() {
                servers.push(config.server.trim().to_string());
            }
            servers.extend(nonempty_unique(config.servers.iter().map(String::as_str)));
            if servers.is_empty() {
                bail!("`nis.server` must be set when `nis.role = \"client\"`");
            }
            servers
        }
        NisRole::ServerOnly => Vec::new(),
    };

    Ok(NisPlan {
        role,
        domain,
        master,
        admin_user: config.admin_user.trim().to_string(),
        client_servers,
        slaves: nonempty_unique(config.slaves.iter().map(String::as_str)),
        push_to_slaves: config.push_to_slaves,
        force_refresh_maps: config.force_refresh_maps,
        local_admin_groups: config.local_admin_groups.clone(),
        packages: packages_for(role),
        services: services_for(role),
        optional_services: optional_services_for(role),
    })
}

fn parse_role(raw: &str) -> anyhow::Result<NisRole> {
    match raw.trim() {
        "master" => Ok(NisRole::Master),
        "slave" => Ok(NisRole::Slave),
        "client" => Ok(NisRole::Client),
        other => bail!("unsupported NIS role `{other}`"),
    }
}

fn packages_for(role: NisRole) -> Vec<&'static str> {
    let mut packages = BTreeSet::new();
    packages.insert("rpcbind");
    packages.insert("yp-tools");
    if role.includes_client() {
        packages.insert("ypbind-mt");
        packages.insert("libnss-nis");
    }
    if role.includes_server() {
        packages.insert("ypserv");
    }
    packages.into_iter().collect()
}

fn services_for(role: NisRole) -> Vec<&'static str> {
    let mut services = Vec::new();
    services.push("rpcbind");
    if role.includes_server() {
        services.push("ypserv");
    }
    if role.includes_client() {
        services.push("ypbind");
    }
    services
}

fn optional_services_for(role: NisRole) -> Vec<&'static str> {
    if role.includes_server() {
        vec!["ypxfrd"]
    } else {
        Vec::new()
    }
}

fn render_yp_conf(domain: &str, servers: &[&str]) -> String {
    let mut lines = vec![
        "# Managed by DebKit.".to_string(),
        "# Local changes may be overwritten.".to_string(),
    ];
    for server in servers {
        lines.push(format!("domain {domain} server {server}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn render_ypservers_source(master: &str, slaves: &[String]) -> String {
    let mut hosts = vec![master.to_string()];
    hosts.extend(slaves.iter().cloned());
    let mut out = nonempty_unique(hosts.iter().map(String::as_str)).join("\n");
    out.push('\n');
    out
}

fn render_ypservers_makedbm_input(master: &str, slaves: &[String]) -> String {
    let mut hosts = vec![master.to_string()];
    hosts.extend(slaves.iter().cloned());
    let mut out = nonempty_unique(hosts.iter().map(String::as_str))
        .into_iter()
        .map(|host| format!("{host}\t{host}"))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

fn ensure_nsswitch_uses_files_then_nis(path: &Path) -> anyhow::Result<bool> {
    let existing = if path.exists() {
        std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };
    let updated = render_nsswitch_with_files_then_nis(&existing);
    if existing == updated {
        return Ok(false);
    }
    ensure_root_file(path, &updated)
}

fn render_nsswitch_with_files_then_nis(raw: &str) -> String {
    let mut saw_passwd = false;
    let mut saw_group = false;
    let mut saw_shadow = false;
    let mut lines = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim_start();
        let replacement = if trimmed.starts_with("passwd:") {
            saw_passwd = true;
            Some("passwd:         files nis")
        } else if trimmed.starts_with("group:") {
            saw_group = true;
            Some("group:          files nis")
        } else if trimmed.starts_with("shadow:") {
            saw_shadow = true;
            Some("shadow:         files nis")
        } else {
            None
        };
        lines.push(replacement.unwrap_or(line).to_string());
    }

    if !saw_passwd {
        lines.push("passwd:         files nis".to_string());
    }
    if !saw_group {
        lines.push("group:          files nis".to_string());
    }
    if !saw_shadow {
        lines.push("shadow:         files nis".to_string());
    }

    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn nsswitch_has_active_initgroups(raw: &str) -> bool {
    raw.lines().any(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with('#') && trimmed.starts_with("initgroups:")
    })
}

fn initialize_master_maps_if_needed(domain: &str) -> anyhow::Result<()> {
    let domain_dir = Path::new(YP_MAP_ROOT).join(domain);
    if !domain_dir.exists() {
        run_privileged_command_with_input(
            YPINIT_PATH,
            &["-m"],
            "",
            "initializing NIS master maps",
        )?;
    }
    Ok(())
}

fn rebuild_master_maps(domain: &str) -> anyhow::Result<()> {
    run_privileged_command(
        "make",
        &["-C", YP_MAP_ROOT],
        &format!("rebuilding NIS master maps for {domain}"),
    )
}

fn ensure_ypservers_source(master: &str, slaves: &[String]) -> anyhow::Result<()> {
    ensure_root_dir(Path::new(YP_MAP_ROOT))?;
    ensure_root_file(
        Path::new(YPSERVERS_SOURCE_PATH),
        &render_ypservers_source(master, slaves),
    )?;
    Ok(())
}

fn rebuild_ypservers_map(domain: &str) -> anyhow::Result<()> {
    let domain_dir = Path::new(YP_MAP_ROOT).join(domain);
    ensure_root_dir(&domain_dir)?;
    let source = std::fs::read_to_string(YPSERVERS_SOURCE_PATH)
        .with_context(|| format!("failed to read {YPSERVERS_SOURCE_PATH}"))?;
    let hosts = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|host| host.to_string())
        .collect::<Vec<_>>();
    let (master, slaves) = hosts
        .split_first()
        .with_context(|| format!("{YPSERVERS_SOURCE_PATH} contains no NIS servers"))?;
    let input = render_ypservers_makedbm_input(master, slaves);
    let tmp = format!("/tmp/debkit-ypservers-{}.map", std::process::id());
    std::fs::write(&tmp, input).with_context(|| format!("failed to write {tmp}"))?;
    let result = run_privileged_command(
        MAKEDBM_PATH,
        &[&tmp, &format!("{}/{domain}/ypservers", YP_MAP_ROOT)],
        "rebuilding NIS ypservers map",
    );
    let _ = std::fs::remove_file(&tmp);
    result
}

fn validate_ypservers_map(domain: &str, master: &str, slaves: &[String]) -> anyhow::Result<()> {
    let map_path = format!("{}/{domain}/ypservers", YP_MAP_ROOT);
    let raw = capture(MAKEDBM_PATH, &["-u", &map_path])
        .with_context(|| format!("failed to inspect generated ypservers map at {map_path}"))?;
    let hosts = nonempty_unique(std::iter::once(master).chain(slaves.iter().map(String::as_str)));
    for host in &hosts {
        let found = raw.lines().any(|line| {
            let mut parts = line.split_whitespace();
            matches!((parts.next(), parts.next()), (Some(key), Some(value)) if key == host && value == host)
        });
        if !found {
            bail!("generated ypservers map is missing non-empty key/value entry for `{host}`");
        }
    }
    let ypcat = capture("ypcat", &["ypservers"])
        .context("failed to validate generated ypservers map with `ypcat ypservers`")?;
    for host in &hosts {
        if !ypcat.lines().any(|line| line.trim() == host) {
            bail!("`ypcat ypservers` did not return `{host}`");
        }
    }
    Ok(())
}

fn initialize_or_refresh_slave_maps(domain: &str, master: &str) -> anyhow::Result<()> {
    let domain_dir = Path::new(YP_MAP_ROOT).join(domain);
    if domain_dir.exists() && domain_dir_has_maps(&domain_dir)? {
        refresh_slave_maps_if_possible(domain)?;
        return Ok(());
    }

    ensure_root_dir(&domain_dir)?;
    match run_privileged_command(
        YPINIT_PATH,
        &["-s", master],
        "initializing NIS slave maps from master",
    ) {
        Ok(()) => Ok(()),
        Err(err) => {
            println!(
                "warning: `/usr/lib/yp/ypinit -s {master}` failed ({err:#}); falling back to direct ypxfr map transfer"
            );
            transfer_maps_manually(domain, master)
        }
    }
}

fn refresh_slave_maps_if_possible(domain: &str) -> anyhow::Result<()> {
    let mut refreshed = false;
    for script in YPXFR_REFRESH_SCRIPTS {
        if Path::new(script).exists() {
            run_privileged_command(
                script,
                &[],
                &format!("refreshing NIS slave maps for {domain}"),
            )?;
            refreshed = true;
        }
    }
    if !refreshed {
        println!(
            "NIS slave maps for `{domain}` already exist; no Debian ypxfr refresh scripts were found, so existing local maps were kept."
        );
    }
    Ok(())
}

fn transfer_maps_manually(domain: &str, master: &str) -> anyhow::Result<()> {
    let maps = maps_from_master(domain)
        .unwrap_or_else(|| FALLBACK_MAPS.iter().map(|map| (*map).to_string()).collect());
    ensure_root_dir(&Path::new(YP_MAP_ROOT).join(domain))?;
    for map in maps {
        run_privileged_command(
            YPXFR_PATH,
            &["-h", master, "-d", domain, &map],
            &format!("transferring NIS map {map} from {master}"),
        )?;
    }
    Ok(())
}

fn force_refresh_slave_maps(domain: &str, master: &str) -> anyhow::Result<()> {
    ensure_root_dir(&Path::new(YP_MAP_ROOT).join(domain))?;
    for map in FALLBACK_MAPS {
        run_privileged_command(
            YPXFR_PATH,
            &["-f", "-h", master, "-d", domain, map],
            &format!("force-refreshing NIS map {map} from {master}"),
        )?;
    }
    Ok(())
}

fn push_maps_to_slaves_if_requested(plan: &NisPlan) -> anyhow::Result<()> {
    if !plan.push_to_slaves {
        return Ok(());
    }
    if plan.slaves.is_empty() {
        println!(
            "warning: `nis.push_to_slaves = true` but `nis.slaves` is empty; no slave pushes attempted"
        );
        return Ok(());
    }
    if !Path::new(YPPUSH_PATH).exists() {
        bail!(
            "`nis.push_to_slaves = true` but {YPPUSH_PATH} is not available; run `debkit configure nis` on each slave to pull maps"
        );
    }
    for slave in &plan.slaves {
        for map in FALLBACK_MAPS {
            run_privileged_command(
                YPPUSH_PATH,
                &["-d", &plan.domain, "-h", slave, map],
                &format!("pushing NIS map {map} to {slave}"),
            )?;
        }
    }
    Ok(())
}

fn maps_from_master(domain: &str) -> Option<Vec<String>> {
    let output = capture("ypwhich", &["-d", domain, "-m"]).ok()?;
    let maps = output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|map| !map.is_empty())
        .map(|map| map.to_string())
        .collect::<Vec<_>>();
    if maps.is_empty() { None } else { Some(maps) }
}

fn domain_dir_has_maps(path: &Path) -> anyhow::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    for entry in
        std::fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read {}", path.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with('.') {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_runtime_nis_domain(domain: &str) -> anyhow::Result<()> {
    run_privileged_command("domainname", &[domain], "setting runtime NIS domain")
}

fn enable_required_services(plan: &NisPlan) -> anyhow::Result<()> {
    for service in &plan.services {
        enable_and_start_service(service)?;
    }
    enable_optional_services(plan)
}

fn enable_optional_services(plan: &NisPlan) -> anyhow::Result<()> {
    for service in &plan.optional_services {
        enable_and_start_service_if_available(service)?;
    }
    Ok(())
}

fn enable_and_start_service(service: &str) -> anyhow::Result<()> {
    run_privileged_command(
        "systemctl",
        &["enable", "--now", service],
        &format!("enabling {service}"),
    )
}

fn enable_and_start_service_if_available(service: &str) -> anyhow::Result<()> {
    if systemd_unit_exists(service) {
        enable_and_start_service(service)?;
    } else {
        println!("warning: optional service `{service}` is not available; skipping");
    }
    Ok(())
}

fn restart_service(service: &str) -> anyhow::Result<()> {
    run_privileged_command(
        "systemctl",
        &["restart", service],
        &format!("restarting {service}"),
    )
}

fn systemd_unit_exists(service: &str) -> bool {
    Path::new(&format!("/usr/lib/systemd/system/{service}.service")).exists()
        || Path::new(&format!("/etc/systemd/system/{service}.service")).exists()
}

fn install_packages(packages: &[&str], label: &str) -> anyhow::Result<()> {
    run_apt_command(&["update"])?;
    let mut args = vec!["install", "-y"];
    args.extend(packages);
    run_apt_command(&args)?;

    for package in packages {
        if !package_installed(package)? {
            bail!("`{package}` was not installed after installing {label}");
        }
    }
    Ok(())
}

fn run_apt_command(args: &[&str]) -> anyhow::Result<()> {
    let euid = current_euid()?;

    let mut command;
    if euid == 0 {
        command = Command::new("apt");
        command.args(args);
    } else if command_available("sudo") {
        command = Command::new("sudo");
        command.arg("apt").args(args);
    } else {
        bail!("installing NIS requires root privileges; run as root or install `sudo` and retry");
    }

    let status = command
        .env("DEBIAN_FRONTEND", "noninteractive")
        .status()
        .context("failed to launch apt")?;
    if !status.success() {
        bail!("apt {} failed with status {}", args.join(" "), status);
    }

    Ok(())
}

fn run_privileged_command(program: &str, args: &[&str], label: &str) -> anyhow::Result<()> {
    run_privileged_command_with_input(program, args, "", label)
}

fn run_privileged_command_with_input(
    program: &str,
    args: &[&str],
    stdin: &str,
    label: &str,
) -> anyhow::Result<()> {
    let euid = current_euid()?;

    let mut command;
    if euid == 0 {
        command = Command::new(program);
        command.args(args);
    } else if command_available("sudo") {
        command = Command::new("sudo");
        command.arg(program).args(args);
    } else {
        bail!("{label} requires root privileges; run as root or install `sudo` and retry");
    }

    let mut child = command
        .stdin(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed while {label}"))?;
    if !stdin.is_empty() {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("piped stdin")
            .write_all(stdin.as_bytes())
            .with_context(|| format!("failed to write stdin while {label}"))?;
    }
    drop(child.stdin.take());

    let status = child
        .wait()
        .with_context(|| format!("failed while {label}"))?;
    if !status.success() {
        bail!("{program} {} failed with status {}", args.join(" "), status);
    }
    Ok(())
}

fn package_installed(package: &str) -> anyhow::Result<bool> {
    let status = Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", package])
        .status()
        .with_context(|| format!("failed to query package `{package}`"))?;
    Ok(status.success())
}

fn ensure_root_dir(path: &Path) -> anyhow::Result<bool> {
    if path.is_dir() {
        return Ok(false);
    }
    if current_euid()? == 0 {
        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        return Ok(true);
    }
    if !command_available("sudo") {
        bail!(
            "creating {} requires root privileges; run as root or install `sudo` and retry",
            path.display()
        );
    }
    let status = Command::new("sudo")
        .arg("mkdir")
        .arg("-p")
        .arg(path)
        .status()
        .with_context(|| format!("failed to create {}", path.display()))?;
    if !status.success() {
        bail!(
            "sudo mkdir -p {} failed with status {}",
            path.display(),
            status
        );
    }
    Ok(true)
}

fn ensure_root_file(path: &Path, content: &str) -> anyhow::Result<bool> {
    if path.exists() {
        let existing = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if existing == content {
            return Ok(false);
        }
    }

    if current_euid()? == 0 {
        std::fs::write(path, content)
            .with_context(|| format!("failed to write {}", path.display()))?;
        return Ok(true);
    }

    if !command_available("sudo") {
        bail!(
            "writing {} requires root privileges; run as root or install `sudo` and retry",
            path.display()
        );
    }

    let status = Command::new("sudo")
        .arg("tee")
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(content.as_bytes())?;
            child.wait()
        })
        .with_context(|| format!("failed to write {}", path.display()))?;
    if !status.success() {
        bail!("sudo tee {} failed with status {}", path.display(), status);
    }
    Ok(true)
}

fn warn_if_no_local_admin(groups: &[String]) {
    let sudo_group = std::fs::read_to_string("/etc/group").ok();
    let has_admin_member = sudo_group
        .as_deref()
        .map(|raw| {
            groups.iter().any(|group| {
                raw.lines()
                    .find(|line| line.starts_with(&format!("{group}:")))
                    .and_then(|line| line.split(':').nth(3))
                    .map(|members| !members.trim().is_empty())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if !has_admin_member {
        println!(
            "warning: no local admin group member was detected in /etc/group; keep at least one local admin account for recovery before relying on NIS."
        );
    }
}

fn validate_nis_admin_group_membership(plan: &NisPlan) {
    let user = plan.admin_user.trim();
    if user.is_empty() {
        return;
    }

    for group in &plan.local_admin_groups {
        let group = group.trim();
        if group.is_empty() {
            continue;
        }

        let group_result = capture_status("getent", &["group", group]);
        let initgroups_result = capture_status("getent", &["initgroups", user]);
        let id_result = capture_status("id", &[user]);

        println!("Validated NIS group lookup commands:");
        println!("  getent group {group}");
        println!("  getent initgroups {user}");
        println!("  id {user}");

        let group_output = match group_result {
            Ok(output) => output,
            Err(err) => {
                println!("warning: `getent group {group}` did not succeed: {err:#}");
                continue;
            }
        };
        let initgroups_output = match initgroups_result {
            Ok(output) => output,
            Err(err) => {
                println!("warning: `getent initgroups {user}` did not succeed: {err:#}");
                String::new()
            }
        };
        let id_output = match id_result {
            Ok(output) => output,
            Err(err) => {
                println!("warning: `id {user}` did not succeed: {err:#}");
                String::new()
            }
        };

        let group_lists_user = group_entry_lists_user(group, user, &group_output);
        let initgroups_lists_group = whitespace_fields_contain(&initgroups_output, group);
        let id_lists_group = id_output_contains_group(&id_output, group);
        if group_lists_user && (!initgroups_lists_group || !id_lists_group) {
            let nsswitch = std::fs::read_to_string(NSSWITCH_PATH).unwrap_or_default();
            if nsswitch_has_active_initgroups(&nsswitch) {
                println!(
                    "warning: NIS group `{group}` lists `{user}`, but supplementary group lookup did not include it; /etc/nsswitch.conf has an active `initgroups:` line, so inspect that line before relying on NIS group membership."
                );
            } else {
                println!(
                    "warning: NIS group `{group}` lists `{user}`, but supplementary group lookup did not include it."
                );
            }
        }
    }
}

fn group_entry_lists_user(group: &str, user: &str, raw: &str) -> bool {
    raw.lines().any(|line| {
        let mut parts = line.split(':');
        let name = parts.next().unwrap_or_default();
        let members = parts.nth(2).unwrap_or_default();
        name == group
            && members
                .split(',')
                .map(str::trim)
                .any(|member| member == user)
    })
}

fn whitespace_fields_contain(raw: &str, expected: &str) -> bool {
    raw.split_whitespace().any(|field| field == expected)
}

fn id_output_contains_group(raw: &str, group: &str) -> bool {
    raw.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .any(|field| field == group)
}

fn command_available(program: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {program} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn capture(program: &str, args: &[&str]) -> anyhow::Result<String> {
    capture_status(program, args)
}

fn capture_status(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program} {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed with status {}",
            args.join(" "),
            output.status
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn current_hostname() -> anyhow::Result<String> {
    Ok(capture("hostname", &[])?.trim().to_string())
}

fn current_fqdn(domain: &str) -> anyhow::Result<String> {
    let fqdn = capture("hostname", &["-f"])
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "(none)");
    if let Some(fqdn) = fqdn {
        return Ok(fqdn);
    }
    let hostname = current_hostname()?;
    if hostname.contains('.') {
        Ok(hostname)
    } else {
        Ok(format!("{hostname}.{domain}"))
    }
}

fn current_euid() -> anyhow::Result<u32> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to run `id -u`")?;
    if !output.status.success() {
        bail!("`id -u` failed with status {}", output.status);
    }

    let stdout = String::from_utf8(output.stdout).context("`id -u` returned non-UTF-8 output")?;
    let trimmed = stdout.trim();
    trimmed
        .parse::<u32>()
        .with_context(|| format!("failed to parse `id -u` output `{trimmed}`"))
}

fn nonempty_unique<'a>(items: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in items {
        let item = item.trim();
        if !item.is_empty() && seen.insert(item.to_string()) {
            out.push(item.to_string());
        }
    }
    out
}

impl NisPlan {
    fn client_servers_as_strs(&self) -> Vec<&str> {
        self.client_servers.iter().map(String::as_str).collect()
    }
}

impl NisRole {
    fn label(self) -> &'static str {
        match self {
            Self::Master => "NIS master",
            Self::Slave => "NIS slave",
            Self::Client => "NIS client",
            Self::ServerOnly => "NIS server",
        }
    }

    fn config_value(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Slave => "slave",
            Self::Client => "client",
            Self::ServerOnly => "server",
        }
    }

    fn includes_client(self) -> bool {
        matches!(self, Self::Master | Self::Slave | Self::Client)
    }

    fn includes_server(self) -> bool {
        matches!(self, Self::Master | Self::Slave | Self::ServerOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config(role: &str) -> NisConfig {
        NisConfig {
            enabled: true,
            role: role.to_string(),
            domain: "example.lan".to_string(),
            admin_user: std::env::var("SUDO_USER")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_default(),
            local_admin_groups: vec!["superuser".to_string()],
            master: "nis-master.example.lan".to_string(),
            server: "nis-master.example.lan".to_string(),
            prefer_local: true,
            push_to_slaves: false,
            force_refresh_maps: false,
            slaves: Vec::new(),
            servers: Vec::new(),
        }
    }

    #[test]
    fn master_installs_server_and_client_pieces() {
        let mut config = base_config("master");
        config.slaves = vec!["nis-slave.example.lan".to_string()];
        config.push_to_slaves = true;
        let plan = build_plan(Role::Configured, &config).unwrap();
        assert_eq!(plan.role, NisRole::Master);
        assert!(plan.packages.contains(&"ypserv"));
        assert!(plan.packages.contains(&"ypbind-mt"));
        assert!(plan.packages.contains(&"libnss-nis"));
        assert_eq!(plan.services, vec!["rpcbind", "ypserv", "ypbind"]);
        assert_eq!(plan.optional_services, vec!["ypxfrd"]);
        assert_eq!(plan.client_servers, vec!["127.0.0.1"]);
        assert_eq!(plan.slaves, vec!["nis-slave.example.lan"]);
        assert!(plan.push_to_slaves);
    }

    #[test]
    fn slave_prefers_local_server_then_master() {
        let plan = build_plan(Role::Configured, &base_config("slave")).unwrap();
        assert_eq!(plan.role, NisRole::Slave);
        assert!(plan.packages.contains(&"ypserv"));
        assert!(plan.packages.contains(&"ypbind-mt"));
        assert_eq!(plan.services, vec!["rpcbind", "ypserv", "ypbind"]);
        assert_eq!(
            plan.client_servers,
            vec!["127.0.0.1", "nis-master.example.lan"]
        );

        let rendered = render_yp_conf(&plan.domain, &plan.client_servers_as_strs());
        assert!(rendered.contains("domain example.lan server 127.0.0.1"));
        assert!(rendered.contains("domain example.lan server nis-master.example.lan"));
    }

    #[test]
    fn slave_plan_can_force_refresh_maps() {
        let mut config = base_config("slave");
        config.force_refresh_maps = true;
        let plan = build_plan(Role::Configured, &config).unwrap();
        assert!(plan.force_refresh_maps);
    }

    #[test]
    fn slave_requires_master() {
        let mut config = base_config("slave");
        config.master.clear();
        let err = build_plan(Role::Configured, &config).unwrap_err();
        assert!(err.to_string().contains("nis.master"));
    }

    #[test]
    fn client_uses_server_without_server_packages() {
        let plan = build_plan(Role::Configured, &base_config("client")).unwrap();
        assert_eq!(plan.role, NisRole::Client);
        assert!(!plan.packages.contains(&"ypserv"));
        assert_eq!(plan.services, vec!["rpcbind", "ypbind"]);
        assert_eq!(plan.client_servers, vec!["nis-master.example.lan"]);
    }

    #[test]
    fn client_requires_server() {
        let mut config = base_config("client");
        config.server.clear();
        let err = build_plan(Role::Configured, &config).unwrap_err();
        assert!(err.to_string().contains("nis.server"));
    }

    #[test]
    fn old_compound_roles_are_rejected() {
        let err = build_plan(Role::Configured, &base_config("slave-client")).unwrap_err();
        assert!(err.to_string().contains("unsupported NIS role"));
    }

    #[test]
    fn unknown_role_fails() {
        let err = build_plan(Role::Configured, &base_config("bad")).unwrap_err();
        assert!(err.to_string().contains("unsupported NIS role"));
    }

    #[test]
    fn configure_rejects_client_role() {
        let err = configure(&base_config("client")).unwrap_err();
        assert!(err.to_string().contains("requires `nis.role = \"master\"`"));
    }

    #[test]
    fn renders_ypservers_source() {
        let rendered = render_ypservers_source(
            "nis-master.example.lan",
            &[
                "nis-slave.example.lan".to_string(),
                "nis-master.example.lan".to_string(),
            ],
        );
        assert_eq!(rendered, "nis-master.example.lan\nnis-slave.example.lan\n");
    }

    #[test]
    fn renders_ypservers_makedbm_input_with_key_value_pairs() {
        let rendered = render_ypservers_makedbm_input(
            "nis-master.example.lan",
            &["nis-slave.example.lan".to_string()],
        );
        assert_eq!(
            rendered,
            "nis-master.example.lan\tnis-master.example.lan\nnis-slave.example.lan\tnis-slave.example.lan\n"
        );
    }

    #[test]
    fn nsswitch_keeps_files_before_nis() {
        let raw = "passwd: nis files systemd\ngroup: files systemd\nhosts: files dns\n";
        let rendered = render_nsswitch_with_files_then_nis(raw);
        assert!(rendered.contains("passwd:         files nis\n"));
        assert!(rendered.contains("group:          files nis\n"));
        assert!(rendered.contains("shadow:         files nis\n"));
        assert!(rendered.contains("hosts: files dns\n"));
    }

    #[test]
    fn nsswitch_render_is_idempotent() {
        let raw =
            "passwd:         files nis\ngroup:          files nis\nshadow:         files nis\n";
        assert_eq!(render_nsswitch_with_files_then_nis(raw), raw);
    }

    #[test]
    fn nsswitch_render_leaves_initgroups_alone() {
        let raw = "passwd: compat\ngroup: compat\ninitgroups: files nis\nshadow: compat\n";
        let rendered = render_nsswitch_with_files_then_nis(raw);
        assert!(rendered.contains("passwd:         files nis\n"));
        assert!(rendered.contains("group:          files nis\n"));
        assert!(rendered.contains("shadow:         files nis\n"));
        assert!(rendered.contains("initgroups: files nis\n"));
        assert!(nsswitch_has_active_initgroups(&rendered));
    }

    #[test]
    fn commented_initgroups_is_not_active() {
        assert!(!nsswitch_has_active_initgroups("# initgroups: files nis\n"));
    }

    #[test]
    fn group_entry_parser_detects_user_membership() {
        let raw = "superuser:x:1010:jallen,alice\n";
        assert!(group_entry_lists_user("superuser", "jallen", raw));
        assert!(!group_entry_lists_user("superuser", "bob", raw));
    }

    #[test]
    fn id_output_parser_detects_group_name() {
        let raw = "uid=1002(jallen) gid=1002(jallen) groups=1002(jallen),1010(superuser)\n";
        assert!(id_output_contains_group(raw, "superuser"));
        assert!(!id_output_contains_group(raw, "wheel"));
    }
}
