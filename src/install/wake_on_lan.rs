use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::config::{DebkitConfig, WakeOnLanConfig};

const WAKE_INFO_DIR: &str = "/var/lib/debkit/wake-on-lan";
const ETHTOOL_SERVICE_PATH: &str = "/etc/systemd/system/debkit-wol@.service";
const ETHTOOL_SERVICE: &str = "[Unit]\nDescription=Enable Wake-on-LAN for %i\nAfter=network.target\n\n[Service]\nType=oneshot\nExecStart=/usr/sbin/ethtool -s %i wol g\n\n[Install]\nWantedBy=multi-user.target\n";

#[derive(Debug, Clone)]
struct HostReport {
    hostname: String,
    os_version: String,
    network_manager_installed: bool,
    network_manager_running: bool,
    default_route_interface: Option<String>,
    ethtool_installed: bool,
    wakeonlan_installed: bool,
    etherwake_installed: bool,
    interfaces: Vec<InterfaceReport>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct InterfaceReport {
    name: String,
    kind: InterfaceKind,
    mac_address: Option<String>,
    permanent_mac_address: Option<String>,
    supports_wake_on: Option<String>,
    ethtool_wake_on: Option<String>,
    link_detected: Option<String>,
    nm_managed: bool,
    nm_connection: Option<String>,
    nm_wake_on_lan: Option<String>,
}

#[derive(Debug, Clone)]
struct WakeInfo {
    hostname: String,
    interface: String,
    mac_address: String,
    wake_mode: String,
    requested_backend: BackendRequest,
    selected_backend: SelectedBackend,
    network_manager_connection: Option<String>,
    network_manager_wake_on_lan: Option<String>,
    ethtool_installed: bool,
    ethtool_wake_on: Option<String>,
    wake_from: String,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InterfaceKind {
    Wired,
    Wireless,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendRequest {
    NetworkManager,
    Ethtool,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectedBackend {
    NetworkManager,
    Ethtool,
}

pub fn run(config: &DebkitConfig) -> anyhow::Result<()> {
    if !config.wake_on_lan.enabled {
        println!("Wake-on-LAN is disabled in config (`wake_on_lan.enabled = false`).");
        println!("Set it to true before running `debkit install wake-on-lan`.");
        print_status(config)?;
        return Ok(());
    }

    let request = parse_backend(&config.wake_on_lan.backend)?;
    let before = collect_report(config)?;
    let selected_interfaces = selected_interfaces(&before, &config.wake_on_lan)?;
    let backend = select_backend(&before, &selected_interfaces, request);

    for iface_name in &selected_interfaces {
        ensure_wired_target(&before, iface_name)?;
    }

    match backend {
        SelectedBackend::NetworkManager => {
            configure_network_manager(&before, &selected_interfaces)?
        }
        SelectedBackend::Ethtool => configure_ethtool(&before, &selected_interfaces)?,
    }

    let mut after = collect_report(config)?;
    after.warnings.extend(backend_warnings(&after, backend));

    let wake_infos = build_wake_infos(&after, &selected_interfaces, request, backend, config)?;
    write_wake_info_files(&wake_infos)?;
    print_configured_summary(&wake_infos);
    Ok(())
}

pub fn dry_run(config: &DebkitConfig) -> anyhow::Result<()> {
    let request = parse_backend(&config.wake_on_lan.backend)?;
    let report = collect_report(config)?;
    let interfaces = selected_interfaces(&report, &config.wake_on_lan)?;
    let backend = select_backend(&report, &interfaces, request);

    println!("Wake-on-LAN dry run");
    println!("interfaces selected: {}", interfaces.join(", "));
    println!("backend requested: {}", request.as_str());
    println!("backend that would be selected: {}", backend.as_str());
    println!(
        "ethtool would be installed: {}",
        yes_no(backend == SelectedBackend::Ethtool && !report.ethtool_installed)
    );
    for iface_name in &interfaces {
        let iface = interface_by_name(&report, iface_name)?;
        println!(
            "NetworkManager connection for {iface_name}: {}",
            iface.nm_connection.as_deref().unwrap_or("n/a")
        );
        if backend == SelectedBackend::NetworkManager {
            if let Some(connection) = iface.nm_connection.as_deref() {
                println!(
                    "would run: nmcli connection modify \"{connection}\" 802-3-ethernet.wake-on-lan magic"
                );
            } else {
                println!("would fail: NetworkManager has no active connection for {iface_name}");
            }
        } else {
            println!("would run: ethtool -s {iface_name} wol g");
            println!("would write: {ETHTOOL_SERVICE_PATH}");
            println!("would enable: debkit-wol@{iface_name}.service");
        }
    }
    println!("wake-info files would be written under: {WAKE_INFO_DIR}");
    for info in build_wake_infos(&report, &interfaces, request, backend, config)? {
        println!("TimeVault wake command: wakeonlan {}", info.mac_address);
    }
    Ok(())
}

pub fn print_status(config: &DebkitConfig) -> anyhow::Result<()> {
    let request = parse_backend(&config.wake_on_lan.backend)?;
    let report = collect_report(config)?;
    let selected_interfaces = selected_interfaces(&report, &config.wake_on_lan).ok();
    let selected_backend = selected_interfaces
        .as_deref()
        .map(|interfaces| select_backend(&report, interfaces, request));

    print_report(
        &report,
        request,
        selected_backend,
        selected_interfaces.as_deref(),
        config,
    );
    Ok(())
}

fn collect_report(config: &DebkitConfig) -> anyhow::Result<HostReport> {
    let hostname = capture("hostname", &[])
        .unwrap_or_else(|_| "unknown".to_string())
        .trim()
        .to_string();
    let ethtool_installed = command_available("ethtool");
    let network_manager_installed = command_available("nmcli");
    let network_manager_running = network_manager_installed && network_manager_running();
    let interfaces = collect_interfaces(ethtool_installed, network_manager_installed)?;

    let mut warnings = vec![
        "BIOS/UEFI must allow Wake-on-LAN or PCIe wake for the target machine.".to_string(),
        "Standard Wake-on-LAN is for wired Ethernet; Wi-Fi interfaces are ignored.".to_string(),
    ];
    if hostname == config.wake_on_lan.reference_host {
        warnings.push(format!(
            "this host is the configured reference host `{}`",
            config.wake_on_lan.reference_host
        ));
    }
    if !ethtool_installed {
        warnings.push(
            "`ethtool` is not installed; hardware-level Wake-on-LAN verification is skipped unless the ethtool backend is selected".to_string(),
        );
    }

    Ok(HostReport {
        hostname,
        os_version: os_version(),
        network_manager_installed,
        network_manager_running,
        default_route_interface: default_route_interface(),
        ethtool_installed,
        wakeonlan_installed: command_available("wakeonlan"),
        etherwake_installed: command_available("etherwake"),
        interfaces,
        warnings,
    })
}

fn collect_interfaces(
    ethtool_installed: bool,
    network_manager_installed: bool,
) -> anyhow::Result<Vec<InterfaceReport>> {
    let mut interfaces = Vec::new();
    for entry in fs::read_dir("/sys/class/net").context("failed to read /sys/class/net")? {
        let entry = entry.context("failed to read /sys/class/net entry")?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "lo" {
            continue;
        }

        let path = entry.path();
        let kind = interface_kind(&path, &name);
        let mac_address = read_trimmed(path.join("address")).ok();
        let ethtool = if ethtool_installed {
            capture("ethtool", &[&name]).ok()
        } else {
            None
        };
        let nm_connection = if network_manager_installed {
            active_nm_connection(&name)
        } else {
            None
        };
        let nm_wake_on_lan = nm_connection
            .as_deref()
            .and_then(network_manager_wake_on_lan);
        let nm_managed = network_manager_installed && nm_device_managed(&name);

        interfaces.push(InterfaceReport {
            name,
            kind,
            mac_address,
            permanent_mac_address: ethtool
                .as_deref()
                .and_then(|raw| ethtool_value(raw, "Permanent address")),
            supports_wake_on: ethtool
                .as_deref()
                .and_then(|raw| ethtool_value(raw, "Supports Wake-on")),
            ethtool_wake_on: ethtool
                .as_deref()
                .and_then(|raw| ethtool_value(raw, "Wake-on")),
            link_detected: ethtool
                .as_deref()
                .and_then(|raw| ethtool_value(raw, "Link detected")),
            nm_managed,
            nm_connection,
            nm_wake_on_lan,
        });
    }
    interfaces.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(interfaces)
}

fn selected_interfaces(
    report: &HostReport,
    config: &WakeOnLanConfig,
) -> anyhow::Result<Vec<String>> {
    if !config.interfaces_auto {
        for iface in &config.interfaces {
            if !report.interfaces.iter().any(|found| found.name == *iface) {
                bail!("configured Wake-on-LAN interface `{iface}` was not found");
            }
        }
        return Ok(config.interfaces.clone());
    }

    if let Some(default_iface) = &report.default_route_interface {
        if report
            .interfaces
            .iter()
            .any(|iface| iface.name == *default_iface && iface.kind == InterfaceKind::Wired)
        {
            return Ok(vec![default_iface.clone()]);
        }
    }

    let wired = report
        .interfaces
        .iter()
        .filter(|iface| iface.kind == InterfaceKind::Wired)
        .map(|iface| iface.name.clone())
        .collect::<Vec<_>>();

    match wired.len() {
        0 => bail!("no wired Ethernet interfaces detected for Wake-on-LAN"),
        1 => Ok(wired),
        _ => bail!(
            "multiple wired interfaces detected ({}); set `wake_on_lan.interfaces` explicitly",
            wired.join(", ")
        ),
    }
}

fn ensure_wired_target(report: &HostReport, iface: &str) -> anyhow::Result<()> {
    let Some(found) = report.interfaces.iter().find(|item| item.name == iface) else {
        bail!("configured Wake-on-LAN interface `{iface}` was not found");
    };
    match found.kind {
        InterfaceKind::Wired => Ok(()),
        InterfaceKind::Wireless => {
            bail!("`{iface}` is wireless; standard Wake-on-LAN requires wired Ethernet")
        }
        InterfaceKind::Other => bail!("`{iface}` is not a physical wired Ethernet interface"),
    }
}

fn select_backend(
    report: &HostReport,
    interfaces: &[String],
    request: BackendRequest,
) -> SelectedBackend {
    match request {
        BackendRequest::NetworkManager => SelectedBackend::NetworkManager,
        BackendRequest::Ethtool => SelectedBackend::Ethtool,
        BackendRequest::Auto => {
            if can_use_network_manager(report, interfaces) {
                SelectedBackend::NetworkManager
            } else {
                SelectedBackend::Ethtool
            }
        }
    }
}

fn can_use_network_manager(report: &HostReport, interfaces: &[String]) -> bool {
    report.network_manager_installed
        && report.network_manager_running
        && interfaces.iter().all(|name| {
            report
                .interfaces
                .iter()
                .find(|iface| iface.name == *name)
                .map(|iface| {
                    iface.kind == InterfaceKind::Wired
                        && iface.nm_managed
                        && iface.nm_connection.is_some()
                })
                .unwrap_or(false)
        })
}

fn configure_network_manager(report: &HostReport, interfaces: &[String]) -> anyhow::Result<()> {
    if !report.network_manager_installed {
        bail!("NetworkManager backend requires `nmcli`, but it is not installed");
    }
    if !report.network_manager_running {
        bail!("NetworkManager backend requires NetworkManager to be running");
    }

    for iface_name in interfaces {
        let iface = interface_by_name(report, iface_name)?;
        if !iface.nm_managed {
            bail!("NetworkManager is not managing `{iface_name}`");
        }
        let connection = iface.nm_connection.as_deref().with_context(|| {
            format!("NetworkManager has no active connection for `{iface_name}`")
        })?;

        if iface.nm_wake_on_lan.as_deref() == Some("magic") {
            println!(
                "NetworkManager connection `{connection}` already configured for Wake-on-LAN magic."
            );
        } else {
            run_privileged(
                "nmcli",
                &[
                    "connection",
                    "modify",
                    connection,
                    "802-3-ethernet.wake-on-lan",
                    "magic",
                ],
            )
            .with_context(|| {
                format!("failed to configure NetworkManager Wake-on-LAN for `{iface_name}`")
            })?;
            println!(
                "Configured NetworkManager connection `{connection}` for Wake-on-LAN magic. Reconnect or reboot if the setting is not immediately reflected."
            );
        }
    }
    Ok(())
}

fn configure_ethtool(report: &HostReport, interfaces: &[String]) -> anyhow::Result<()> {
    if !report.ethtool_installed {
        ensure_ethtool_installed()?;
    }

    for iface_name in interfaces {
        let iface = interface_by_name(report, iface_name)?;
        if let Some(supports) = &iface.supports_wake_on {
            if !supports.split_whitespace().any(|mode| mode == "g") {
                bail!(
                    "`{iface_name}` does not report magic-packet support (`Supports Wake-on: {supports}`)"
                );
            }
        }
        run_privileged("ethtool", &["-s", iface_name, "wol", "g"])
            .with_context(|| format!("failed to enable runtime Wake-on-LAN on `{iface_name}`"))?;
    }

    let changed = ensure_root_file(Path::new(ETHTOOL_SERVICE_PATH), ETHTOOL_SERVICE)?;
    if changed {
        run_privileged("systemctl", &["daemon-reload"])?;
    }
    for iface_name in interfaces {
        run_privileged(
            "systemctl",
            &["enable", &format!("debkit-wol@{iface_name}.service")],
        )?;
    }

    let after = collect_interfaces(true, command_available("nmcli"))?;
    for iface_name in interfaces {
        let wake_on = after
            .iter()
            .find(|iface| iface.name == *iface_name)
            .and_then(|iface| iface.ethtool_wake_on.as_deref())
            .unwrap_or("unknown");
        if wake_on != "g" {
            bail!("ethtool verification failed for `{iface_name}` (`Wake-on: {wake_on}`)");
        }
    }

    Ok(())
}

fn build_wake_infos(
    report: &HostReport,
    interfaces: &[String],
    request: BackendRequest,
    backend: SelectedBackend,
    config: &DebkitConfig,
) -> anyhow::Result<Vec<WakeInfo>> {
    let mut infos = Vec::new();
    for iface_name in interfaces {
        let iface = interface_by_name(report, iface_name)?;
        let mac = iface
            .mac_address
            .clone()
            .with_context(|| format!("failed to determine MAC address for `{iface_name}`"))?;
        let mut warnings = report.warnings.clone();
        if backend == SelectedBackend::NetworkManager && !report.ethtool_installed {
            warnings.push("NetworkManager setting was verified, but hardware-level ethtool verification was skipped because ethtool is not installed.".to_string());
        }
        if report.ethtool_installed {
            match iface.ethtool_wake_on.as_deref() {
                Some("g") => {}
                Some(state) => warnings.push(format!(
                    "ethtool hardware validation for `{iface_name}` reported `Wake-on: {state}` instead of `g`"
                )),
                None => warnings.push(format!(
                    "ethtool is installed, but hardware Wake-on-LAN state for `{iface_name}` could not be read"
                )),
            }
        }
        infos.push(WakeInfo {
            hostname: report.hostname.clone(),
            interface: iface.name.clone(),
            mac_address: mac,
            wake_mode: config.wake_on_lan.mode.clone(),
            requested_backend: request,
            selected_backend: backend,
            network_manager_connection: iface.nm_connection.clone(),
            network_manager_wake_on_lan: iface.nm_wake_on_lan.clone(),
            ethtool_installed: report.ethtool_installed,
            ethtool_wake_on: iface.ethtool_wake_on.clone(),
            wake_from: "timevault".to_string(),
            warnings,
        });
    }
    Ok(infos)
}

fn write_wake_info_files(infos: &[WakeInfo]) -> anyhow::Result<()> {
    ensure_root_dir(Path::new(WAKE_INFO_DIR))?;
    for info in infos {
        let stem = if infos.len() == 1 {
            info.hostname.clone()
        } else {
            format!("{}-{}", info.hostname, info.interface)
        };
        ensure_root_file(
            &Path::new(WAKE_INFO_DIR).join(format!("{stem}.txt")),
            &render_wake_info_text(info),
        )?;
        ensure_root_file(
            &Path::new(WAKE_INFO_DIR).join(format!("{stem}.json")),
            &render_wake_info_json(info),
        )?;
    }
    Ok(())
}

fn print_report(
    report: &HostReport,
    request: BackendRequest,
    selected_backend: Option<SelectedBackend>,
    selected_interfaces: Option<&[String]>,
    config: &DebkitConfig,
) {
    println!("Wake-on-LAN report");
    println!("hostname: {}", report.hostname);
    println!("OS version: {}", report.os_version);
    println!("requested backend: {}", request.as_str());
    println!(
        "selected backend: {}",
        selected_backend
            .map(|backend| backend.as_str())
            .unwrap_or("not selected")
    );
    println!(
        "NetworkManager installed/running: {}/{}",
        yes_no(report.network_manager_installed),
        yes_no(report.network_manager_running)
    );
    println!("ethtool installed: {}", yes_no(report.ethtool_installed));
    println!(
        "wakeonlan installed: {}",
        yes_no(report.wakeonlan_installed)
    );
    println!(
        "etherwake installed: {}",
        yes_no(report.etherwake_installed)
    );
    println!(
        "default route interface: {}",
        report
            .default_route_interface
            .as_deref()
            .unwrap_or("unknown")
    );

    println!("\nInterfaces:");
    for iface in &report.interfaces {
        let selected = selected_interfaces
            .map(|items| items.iter().any(|name| name == &iface.name))
            .unwrap_or(false);
        println!(
            "- {} ({}){}",
            iface.name,
            iface.kind.as_str(),
            if selected { " [target]" } else { "" }
        );
        println!("  MAC address: {}", opt(&iface.mac_address));
        println!(
            "  permanent MAC address: {}",
            opt(&iface.permanent_mac_address)
        );
        println!("  NetworkManager managed: {}", yes_no(iface.nm_managed));
        println!(
            "  active NetworkManager connection: {}",
            opt(&iface.nm_connection)
        );
        println!(
            "  NetworkManager wake-on-lan: {}",
            opt(&iface.nm_wake_on_lan)
        );
        println!("  Supports Wake-on: {}", opt(&iface.supports_wake_on));
        println!("  ethtool Wake-on: {}", opt(&iface.ethtool_wake_on));
        println!("  link detected: {}", opt(&iface.link_detected));
        if iface.kind == InterfaceKind::Wireless {
            println!("  warning: ignored for standard Wake-on-LAN");
        }
    }

    if let Some(interfaces) = selected_interfaces {
        match build_wake_infos(
            report,
            interfaces,
            request,
            selected_backend.unwrap_or(SelectedBackend::NetworkManager),
            config,
        ) {
            Ok(infos) => {
                println!("\nTimeVault wake information:");
                for info in infos {
                    println!("- target hostname: {}", info.hostname);
                    println!("  target wired interface name: {}", info.interface);
                    println!("  target MAC address: {}", info.mac_address);
                    println!("  wake command: wakeonlan {}", info.mac_address);
                    println!("  optional etherwake: {}", etherwake_command(&info));
                }
            }
            Err(err) => println!("\nTimeVault wake information unavailable: {err:#}"),
        }
    }

    println!("\nWarnings:");
    for warning in &report.warnings {
        println!("- {warning}");
    }
}

fn print_configured_summary(infos: &[WakeInfo]) {
    for info in infos {
        println!("\nWake-on-LAN configured:");
        println!("  Hostname: {}", info.hostname);
        println!("  Interface: {}", info.interface);
        println!("  MAC: {}", info.mac_address);
        println!("  Requested backend: {}", info.requested_backend.as_str());
        println!("  Selected backend: {}", info.selected_backend.as_str());
        println!(
            "  NetworkManager connection: {}",
            info.network_manager_connection.as_deref().unwrap_or("n/a")
        );
        println!(
            "  ethtool verification: {}",
            match info.ethtool_wake_on.as_deref() {
                Some("g") => "Wake-on: g",
                Some(_) => "failed",
                None if info.ethtool_installed => "unknown",
                None => "skipped",
            }
        );
        println!(
            "\nFrom {}, wake it with:\n  wakeonlan {}\n\nOr:\n  {}",
            info.wake_from,
            info.mac_address,
            etherwake_command(info)
        );
    }
}

fn render_wake_info_text(info: &WakeInfo) -> String {
    format!(
        "Hostname: {}\nInterface: {}\nMAC: {}\nWake mode: {}\nRequested backend: {}\nSelected backend: {}\nNetworkManager connection: {}\nNetworkManager wake-on-lan: {}\nethtool installed: {}\nethtool Wake-on: {}\nWake from: {}\n\nWake commands:\n  wakeonlan {}\n  {}\n\nWarnings:\n{}\n",
        info.hostname,
        info.interface,
        info.mac_address,
        info.wake_mode,
        info.requested_backend.as_str(),
        info.selected_backend.as_str(),
        info.network_manager_connection.as_deref().unwrap_or("null"),
        info.network_manager_wake_on_lan
            .as_deref()
            .unwrap_or("null"),
        info.ethtool_installed,
        info.ethtool_wake_on.as_deref().unwrap_or("null"),
        info.wake_from,
        info.mac_address,
        etherwake_command(info),
        info.warnings
            .iter()
            .map(|warning| format!("- {warning}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn render_wake_info_json(info: &WakeInfo) -> String {
    format!(
        "{{\n  \"hostname\": {},\n  \"interface\": {},\n  \"mac_address\": {},\n  \"wake_mode\": {},\n  \"requested_backend\": {},\n  \"selected_backend\": {},\n  \"network_manager_connection\": {},\n  \"network_manager_wake_on_lan\": {},\n  \"ethtool_installed\": {},\n  \"ethtool_wake_on\": {},\n  \"wake_from\": {},\n  \"wake_commands\": {{\n    \"wakeonlan\": {},\n    \"etherwake\": {}\n  }},\n  \"warnings\": [{}]\n}}\n",
        json_string(&info.hostname),
        json_string(&info.interface),
        json_string(&info.mac_address),
        json_string(&info.wake_mode),
        json_string(info.requested_backend.as_str()),
        json_string(info.selected_backend.as_str()),
        json_opt(info.network_manager_connection.as_deref()),
        json_opt(info.network_manager_wake_on_lan.as_deref()),
        info.ethtool_installed,
        json_opt(info.ethtool_wake_on.as_deref()),
        json_string(&info.wake_from),
        json_string(&format!("wakeonlan {}", info.mac_address)),
        json_string(&etherwake_command(info)),
        info.warnings
            .iter()
            .map(|warning| json_string(warning))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn etherwake_command(info: &WakeInfo) -> String {
    format!(
        "sudo etherwake -i <timevault-interface> {}",
        info.mac_address
    )
}

fn backend_warnings(report: &HostReport, backend: SelectedBackend) -> Vec<String> {
    if backend == SelectedBackend::NetworkManager && !report.ethtool_installed {
        vec!["hardware-level verification skipped because ethtool is not installed".to_string()]
    } else {
        Vec::new()
    }
}

fn interface_by_name<'a>(
    report: &'a HostReport,
    name: &str,
) -> anyhow::Result<&'a InterfaceReport> {
    report
        .interfaces
        .iter()
        .find(|iface| iface.name == name)
        .with_context(|| format!("interface `{name}` was not found"))
}

fn parse_backend(raw: &str) -> anyhow::Result<BackendRequest> {
    match raw {
        "network_manager" | "networkmanager" => Ok(BackendRequest::NetworkManager),
        "ethtool" => Ok(BackendRequest::Ethtool),
        "auto" => Ok(BackendRequest::Auto),
        other => bail!("unsupported Wake-on-LAN backend `{other}`"),
    }
}

fn interface_kind(path: &Path, name: &str) -> InterfaceKind {
    if path.join("wireless").exists() {
        InterfaceKind::Wireless
    } else if path.join("device").exists() || name.starts_with("en") || name.starts_with("eth") {
        InterfaceKind::Wired
    } else {
        InterfaceKind::Other
    }
}

fn network_manager_running() -> bool {
    systemctl_is_active("NetworkManager") || capture("nmcli", &["general", "status"]).is_ok()
}

fn systemctl_is_active(unit: &str) -> bool {
    Command::new("systemctl")
        .arg("is-active")
        .arg("--quiet")
        .arg(unit)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn default_route_interface() -> Option<String> {
    let raw = capture("ip", &["route", "show", "default"]).ok()?;
    raw.lines().find_map(|line| {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        parts
            .windows(2)
            .find(|pair| pair[0] == "dev")
            .map(|pair| pair[1].to_string())
    })
}

fn active_nm_connection(iface: &str) -> Option<String> {
    capture(
        "nmcli",
        &["-t", "-g", "GENERAL.CONNECTION", "device", "show", iface],
    )
    .ok()
    .map(|raw| raw.trim().to_string())
    .filter(|value| !value.is_empty() && value != "--")
}

fn nm_device_managed(iface: &str) -> bool {
    capture(
        "nmcli",
        &["-t", "-g", "GENERAL.STATE", "device", "show", iface],
    )
    .ok()
    .map(|raw| !raw.to_lowercase().contains("unmanaged"))
    .unwrap_or(false)
}

fn network_manager_wake_on_lan(connection: &str) -> Option<String> {
    capture(
        "nmcli",
        &[
            "-t",
            "-g",
            "802-3-ethernet.wake-on-lan",
            "connection",
            "show",
            connection,
        ],
    )
    .ok()
    .map(|raw| raw.trim().to_string())
    .filter(|value| !value.is_empty())
}

fn os_version() -> String {
    let Ok(raw) = fs::read_to_string("/etc/os-release") else {
        return "unknown".to_string();
    };
    for line in raw.lines() {
        if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
            return value.trim_matches('"').to_string();
        }
    }
    "unknown".to_string()
}

fn ethtool_value(raw: &str, key: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let trimmed = line.trim();
        let (found, value) = trimmed.split_once(':')?;
        if found == key {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn ensure_ethtool_installed() -> anyhow::Result<()> {
    if command_available("ethtool") {
        return Ok(());
    }
    run_apt_command(&["update"])?;
    run_apt_command(&["install", "-y", "ethtool"])?;
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
        bail!(
            "installing packages requires root privileges; run as root or install `sudo` and retry"
        );
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

fn ensure_root_dir(path: &Path) -> anyhow::Result<()> {
    if current_euid()? == 0 {
        fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
        return Ok(());
    }
    run_privileged("mkdir", &["-p", path.to_string_lossy().as_ref()])
}

fn ensure_root_file(path: &Path, content: &str) -> anyhow::Result<bool> {
    if path.exists() {
        let existing = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if existing == content {
            return Ok(false);
        }
    }

    if current_euid()? == 0 {
        fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
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

fn run_privileged(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let euid = current_euid()?;
    let mut command;
    if euid == 0 {
        command = Command::new(program);
        command.args(args);
    } else if command_available("sudo") {
        command = Command::new("sudo");
        command.arg(program).args(args);
    } else {
        bail!("`{program}` requires root privileges; run as root or install `sudo` and retry");
    }

    let status = command
        .status()
        .with_context(|| format!("failed to launch {program}"))?;
    if !status.success() {
        bail!(
            "{} {} failed with status {}",
            program,
            args.join(" "),
            status
        );
    }
    Ok(())
}

fn capture(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    if !output.status.success() {
        bail!(
            "{} {} failed with status {}",
            program,
            args.join(" "),
            output.status
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn command_available(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn read_trimmed(path: PathBuf) -> anyhow::Result<String> {
    fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))
        .map(|value| value.trim().to_string())
}

fn current_euid() -> anyhow::Result<u32> {
    let status = fs::read_to_string("/proc/self/status").context("failed to read effective uid")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let mut fields = rest.split_whitespace();
            let _real = fields.next();
            let Some(effective) = fields.next() else {
                break;
            };
            return effective
                .parse::<u32>()
                .context("failed to parse effective uid");
        }
    }
    bail!("failed to determine effective uid")
}

fn json_string(raw: &str) -> String {
    format!("\"{}\"", raw.replace('\\', "\\\\").replace('"', "\\\""))
}

fn json_opt(value: Option<&str>) -> String {
    value.map(json_string).unwrap_or_else(|| "null".to_string())
}

fn opt(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("unknown")
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

impl BackendRequest {
    fn as_str(self) -> &'static str {
        match self {
            Self::NetworkManager => "network_manager",
            Self::Ethtool => "ethtool",
            Self::Auto => "auto",
        }
    }
}

impl SelectedBackend {
    fn as_str(self) -> &'static str {
        match self {
            Self::NetworkManager => "network_manager",
            Self::Ethtool => "ethtool",
        }
    }
}

impl InterfaceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Wired => "wired",
            Self::Wireless => "wireless",
            Self::Other => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, kind: InterfaceKind) -> InterfaceReport {
        InterfaceReport {
            name: name.to_string(),
            kind,
            mac_address: Some("aa:bb:cc:dd:ee:ff".to_string()),
            permanent_mac_address: None,
            supports_wake_on: Some("pumbg".to_string()),
            ethtool_wake_on: Some("g".to_string()),
            link_detected: Some("yes".to_string()),
            nm_managed: false,
            nm_connection: None,
            nm_wake_on_lan: None,
        }
    }

    fn report(interfaces: Vec<InterfaceReport>) -> HostReport {
        HostReport {
            hostname: "host1".to_string(),
            os_version: "Debian".to_string(),
            network_manager_installed: false,
            network_manager_running: false,
            default_route_interface: None,
            ethtool_installed: false,
            wakeonlan_installed: false,
            etherwake_installed: false,
            interfaces,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn parses_ethtool_values() {
        let raw =
            "Settings for enp5s0:\n\tSupports Wake-on: pumbg\n\tWake-on: d\n\tLink detected: yes\n";
        assert_eq!(
            ethtool_value(raw, "Supports Wake-on").as_deref(),
            Some("pumbg")
        );
        assert_eq!(ethtool_value(raw, "Wake-on").as_deref(), Some("d"));
        assert_eq!(ethtool_value(raw, "Link detected").as_deref(), Some("yes"));
    }

    #[test]
    fn auto_backend_prefers_network_manager_when_ready() {
        let mut en = iface("enp1s0", InterfaceKind::Wired);
        en.nm_managed = true;
        en.nm_connection = Some("Wired".to_string());
        let mut report = report(vec![en]);
        report.network_manager_installed = true;
        report.network_manager_running = true;
        assert_eq!(
            select_backend(&report, &["enp1s0".to_string()], BackendRequest::Auto),
            SelectedBackend::NetworkManager
        );
    }

    #[test]
    fn auto_backend_falls_back_to_ethtool() {
        let report = report(vec![iface("enp1s0", InterfaceKind::Wired)]);
        assert_eq!(
            select_backend(&report, &["enp1s0".to_string()], BackendRequest::Auto),
            SelectedBackend::Ethtool
        );
    }

    #[test]
    fn explicit_backends_are_honored() {
        let report = report(vec![iface("enp1s0", InterfaceKind::Wired)]);
        assert_eq!(
            select_backend(
                &report,
                &["enp1s0".to_string()],
                BackendRequest::NetworkManager
            ),
            SelectedBackend::NetworkManager
        );
        assert_eq!(
            select_backend(&report, &["enp1s0".to_string()], BackendRequest::Ethtool),
            SelectedBackend::Ethtool
        );
    }

    #[test]
    fn automatic_interface_detection_uses_single_wired_interface() {
        let report = report(vec![
            iface("wlp1s0", InterfaceKind::Wireless),
            iface("enp1s0", InterfaceKind::Wired),
        ]);
        let config = WakeOnLanConfig {
            interfaces_auto: true,
            ..WakeOnLanConfig::default()
        };
        assert_eq!(
            selected_interfaces(&report, &config).unwrap(),
            vec!["enp1s0"]
        );
    }

    #[test]
    fn multiple_wired_interfaces_require_config_without_default_route() {
        let report = report(vec![
            iface("enp1s0", InterfaceKind::Wired),
            iface("enp2s0", InterfaceKind::Wired),
        ]);
        let config = WakeOnLanConfig {
            interfaces_auto: true,
            ..WakeOnLanConfig::default()
        };
        assert!(selected_interfaces(&report, &config).is_err());
    }

    #[test]
    fn explicit_interface_is_validated() {
        let report = report(vec![iface("enp1s0", InterfaceKind::Wired)]);
        let config = WakeOnLanConfig {
            interfaces_auto: false,
            interfaces: vec!["enp1s0".to_string()],
            ..WakeOnLanConfig::default()
        };
        assert_eq!(
            selected_interfaces(&report, &config).unwrap(),
            vec!["enp1s0"]
        );
    }

    #[test]
    fn wifi_is_rejected_as_target() {
        let report = report(vec![iface("wlp1s0", InterfaceKind::Wireless)]);
        assert!(ensure_wired_target(&report, "wlp1s0").is_err());
    }

    #[test]
    fn renders_json_wake_info() {
        let info = WakeInfo {
            hostname: "host1".to_string(),
            interface: "enp1s0".to_string(),
            mac_address: "aa:bb:cc:dd:ee:ff".to_string(),
            wake_mode: "magic".to_string(),
            requested_backend: BackendRequest::NetworkManager,
            selected_backend: SelectedBackend::NetworkManager,
            network_manager_connection: Some("Wired".to_string()),
            network_manager_wake_on_lan: Some("magic".to_string()),
            ethtool_installed: false,
            ethtool_wake_on: None,
            wake_from: "timevault".to_string(),
            warnings: vec!["BIOS required".to_string()],
        };
        let json = render_wake_info_json(&info);
        assert!(json.contains("\"hostname\": \"host1\""));
        assert!(json.contains("\"wakeonlan\": \"wakeonlan aa:bb:cc:dd:ee:ff\""));
        assert!(json.contains(
            "\"etherwake\": \"sudo etherwake -i <timevault-interface> aa:bb:cc:dd:ee:ff\""
        ));
        assert!(json.contains("\"ethtool_wake_on\": null"));
        assert!(!json.contains("wake_from_interface"));
    }

    #[test]
    fn renders_text_wake_info() {
        let info = WakeInfo {
            hostname: "host1".to_string(),
            interface: "enp1s0".to_string(),
            mac_address: "aa:bb:cc:dd:ee:ff".to_string(),
            wake_mode: "magic".to_string(),
            requested_backend: BackendRequest::Ethtool,
            selected_backend: SelectedBackend::Ethtool,
            network_manager_connection: None,
            network_manager_wake_on_lan: None,
            ethtool_installed: true,
            ethtool_wake_on: Some("g".to_string()),
            wake_from: "timevault".to_string(),
            warnings: Vec::new(),
        };
        let text = render_wake_info_text(&info);
        assert!(text.contains("Wake commands:"));
        assert!(text.contains("sudo etherwake -i <timevault-interface> aa:bb:cc:dd:ee:ff"));
    }

    #[test]
    fn wake_info_warns_when_ethtool_validation_fails() {
        let mut en = iface("enp1s0", InterfaceKind::Wired);
        en.ethtool_wake_on = Some("d".to_string());
        let mut report = report(vec![en]);
        report.ethtool_installed = true;
        let config = DebkitConfig::default();

        let infos = build_wake_infos(
            &report,
            &["enp1s0".to_string()],
            BackendRequest::NetworkManager,
            SelectedBackend::NetworkManager,
            &config,
        )
        .unwrap();

        assert!(
            infos[0]
                .warnings
                .iter()
                .any(|warning| warning.contains("Wake-on: d"))
        );
    }

    #[test]
    fn ethtool_service_template_matches_policy() {
        assert!(ETHTOOL_SERVICE.contains("Description=Enable Wake-on-LAN for %i"));
        assert!(ETHTOOL_SERVICE.contains("ExecStart=/usr/sbin/ethtool -s %i wol g"));
    }
}
