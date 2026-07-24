//! Best-effort collection of the client's SOURCE network context: default
//! route interface + gateway, interface kind (WiFi vs ethernet vs virtual),
//! MTU, actual egress IP toward the target, a conservative VPN heuristic and
//! IPv6 availability.
//!
//! Everything here is best-effort: any failure yields `None` fields and never
//! aborts a run. The collection idiom mirrors `HostInfo::collect_local()` in
//! `metrics.rs` — standard files on Linux (`/proc`, `/sys`), standard CLI
//! tools on macOS (`route`, `networksetup`, `ifconfig`), and `route print`
//! on Windows. No heavyweight dependencies.
//!
//! Platform coverage:
//! * Linux   — interface, kind, MTU, gateway, local IP, VPN, IPv6.
//! * macOS   — interface, kind, MTU, gateway, local IP, VPN, IPv6.
//! * Windows — gateway + local IP only (interface name/kind/MTU stay `None`).

use crate::metrics::NetworkContext;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs, UdpSocket};

impl NetworkContext {
    /// Collect the source-network context toward `target_host:target_port`.
    ///
    /// Never panics; fields that cannot be determined are left `None`.
    pub fn collect(target_host: &str, target_port: u16) -> Self {
        let (default_interface, gateway_ip) = detect_default_route();
        let interface_kind = default_interface
            .as_deref()
            .map(|iface| detect_interface_kind(iface).to_string());
        let mtu = default_interface.as_deref().and_then(detect_mtu);
        let (vpn_detected, vpn_interface) = detect_vpn(default_interface.as_deref());
        NetworkContext {
            default_interface,
            interface_kind,
            mtu,
            local_ip: detect_local_ip(target_host, target_port).map(|ip| ip.to_string()),
            gateway_ip,
            vpn_detected,
            vpn_interface,
            ipv6_available: detect_ipv6_available(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Egress (local) IP toward the target
// ─────────────────────────────────────────────────────────────────────────────

/// Source address of a UDP socket connect()ed to the target. `connect` on a
/// UDP socket only performs a route lookup — no packets are sent — so this is
/// the address of the interface the OS would actually egress from for the run.
fn detect_local_ip(target_host: &str, target_port: u16) -> Option<IpAddr> {
    // `url::Url::host_str()` keeps brackets around IPv6 literals; strip them
    // so `ToSocketAddrs` can parse the address directly.
    let host = target_host.trim_start_matches('[').trim_end_matches(']');
    let remote = (host, target_port).to_socket_addrs().ok()?.next()?;
    let bind_addr = if remote.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let socket = UdpSocket::bind(bind_addr).ok()?;
    socket.connect(remote).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

// ─────────────────────────────────────────────────────────────────────────────
// Default route (interface + gateway)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `(default_interface, gateway_ip)`, both best-effort.
fn detect_default_route() -> (Option<String>, Option<String>) {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/net/route") {
            Ok(content) => parse_proc_net_route(&content),
            Err(_) => (None, None),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let out = match std::process::Command::new("route")
            .args(["-n", "get", "default"])
            .output()
        {
            Ok(out) => out,
            Err(_) => return (None, None),
        };
        parse_route_get_output(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(target_os = "windows")]
    {
        let out = match std::process::Command::new("route")
            .args(["print", "-4", "0.0.0.0"])
            .output()
        {
            Ok(out) => out,
            Err(_) => return (None, None),
        };
        // Windows only cheaply exposes the gateway + interface *IP* here; the
        // interface *name* would need index mapping — left None (best-effort).
        (
            None,
            parse_route_print_gateway(&String::from_utf8_lossy(&out.stdout)),
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        (None, None)
    }
}

/// Parse Linux `/proc/net/route`: pick the up default route (destination
/// `00000000`) with the lowest metric. Addresses are little-endian hex u32.
pub fn parse_proc_net_route(content: &str) -> (Option<String>, Option<String>) {
    const RTF_UP: u32 = 0x1;
    let mut best: Option<(u32, String, Option<String>)> = None; // (metric, iface, gw)
    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 || fields[1] != "00000000" {
            continue;
        }
        let flags = u32::from_str_radix(fields[3], 16).unwrap_or(0);
        if flags & RTF_UP == 0 {
            continue;
        }
        let metric: u32 = fields[6].parse().unwrap_or(u32::MAX);
        let gateway = u32::from_str_radix(fields[2], 16)
            .ok()
            .filter(|&v| v != 0)
            .map(|v| Ipv4Addr::from(v.swap_bytes()).to_string());
        if best.as_ref().is_none_or(|(m, _, _)| metric < *m) {
            best = Some((metric, fields[0].to_string(), gateway));
        }
    }
    match best {
        Some((_, iface, gw)) => (Some(iface), gw),
        None => (None, None),
    }
}

/// Parse macOS/BSD `route -n get default` output (`gateway:` / `interface:`
/// lines). Returns `(interface, gateway)`.
pub fn parse_route_get_output(output: &str) -> (Option<String>, Option<String>) {
    let mut iface = None;
    let mut gateway = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("interface:") {
            let v = v.trim();
            if !v.is_empty() {
                iface = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("gateway:") {
            let v = v.trim();
            // `gateway: link#12` style entries are not IP gateways.
            if !v.is_empty() && !v.starts_with("link#") {
                gateway = Some(v.to_string());
            }
        }
    }
    (iface, gateway)
}

/// Parse Windows `route print -4 0.0.0.0` output: the active-routes row whose
/// first two columns are `0.0.0.0` carries the gateway in the third column
/// (`On-link` rows are skipped).
pub fn parse_route_print_gateway(output: &str) -> Option<String> {
    for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 4
            && fields[0] == "0.0.0.0"
            && fields[1] == "0.0.0.0"
            && fields[2].parse::<Ipv4Addr>().is_ok()
        {
            return Some(fields[2].to_string());
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Interface kind (ethernet | wifi | virtual | unknown)
// ─────────────────────────────────────────────────────────────────────────────

/// Interface name prefixes that indicate a virtual/tunnel interface on any OS.
const VIRTUAL_PREFIXES: &[&str] = &[
    "utun",
    "tun",
    "tap",
    "wg",
    "ppp",
    "ipsec",
    "tailscale",
    "zt",
    "nordlynx",
    "bridge",
    "br-",
    "docker",
    "veth",
    "vbox",
    "vmnet",
    "virbr",
    "lo",
];

/// True when the interface name looks like a virtual/tunnel device.
pub fn is_virtual_name(iface: &str) -> bool {
    VIRTUAL_PREFIXES.iter().any(|p| iface.starts_with(p))
}

fn detect_interface_kind(iface: &str) -> &'static str {
    if is_virtual_name(iface) {
        return "virtual";
    }
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new(&format!("/sys/class/net/{iface}/wireless")).exists() {
            return "wifi";
        }
        if std::path::Path::new(&format!("/sys/devices/virtual/net/{iface}")).exists() {
            return "virtual";
        }
        if std::path::Path::new(&format!("/sys/class/net/{iface}")).exists() {
            return "ethernet";
        }
        "unknown"
    }
    #[cfg(target_os = "macos")]
    {
        let out = match std::process::Command::new("networksetup")
            .arg("-listallhardwareports")
            .output()
        {
            Ok(out) => out,
            Err(_) => return "unknown",
        };
        classify_hardware_port(&String::from_utf8_lossy(&out.stdout), iface)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        "unknown"
    }
}

/// Classify an interface from macOS `networksetup -listallhardwareports`
/// output ("Hardware Port: Wi-Fi" / "Device: en0" pairs).
pub fn classify_hardware_port(output: &str, iface: &str) -> &'static str {
    let mut current_port: Option<&str> = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Hardware Port:") {
            current_port = Some(v.trim());
        } else if let Some(v) = line.strip_prefix("Device:") {
            if v.trim() == iface {
                let port = current_port.unwrap_or_default().to_ascii_lowercase();
                if port.contains("wi-fi") || port.contains("airport") {
                    return "wifi";
                }
                if port.contains("bridge") {
                    return "virtual";
                }
                if port.contains("ethernet") || port.contains("lan") {
                    return "ethernet";
                }
                return "unknown";
            }
        }
    }
    "unknown"
}

// ─────────────────────────────────────────────────────────────────────────────
// MTU
// ─────────────────────────────────────────────────────────────────────────────

fn detect_mtu(iface: &str) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/sys/class/net/{iface}/mtu"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("ifconfig")
            .arg(iface)
            .output()
            .ok()?;
        parse_ifconfig_mtu(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = iface;
        None
    }
}

/// Parse `mtu <N>` out of BSD/macOS `ifconfig <iface>` output, e.g.
/// `en0: flags=8863<UP,...> mtu 1500`.
pub fn parse_ifconfig_mtu(output: &str) -> Option<u32> {
    let mut tokens = output.split_whitespace();
    while let Some(tok) = tokens.next() {
        if tok == "mtu" {
            return tokens.next()?.parse().ok();
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// VPN heuristic
// ─────────────────────────────────────────────────────────────────────────────

/// Tunnel-like name prefixes for the VPN heuristic — deliberately narrower
/// than [`VIRTUAL_PREFIXES`] (docker/bridge interfaces are not VPNs).
const VPN_PREFIXES: &[&str] = &[
    "utun",
    "tun",
    "tap",
    "wg",
    "ppp",
    "ipsec",
    "tailscale",
    "zt",
    "nordlynx",
];

/// True when the interface name looks like a VPN tunnel device.
pub fn is_vpn_like_name(iface: &str) -> bool {
    VPN_PREFIXES.iter().any(|p| iface.starts_with(p))
}

/// Conservative VPN detection: only claims `Some(true)` when the default
/// route runs through a tunnel-like interface. Unknown default route → `None`
/// (never guesses — false positives are worse than absent data).
fn detect_vpn(default_iface: Option<&str>) -> (Option<bool>, Option<String>) {
    match default_iface {
        Some(iface) if is_vpn_like_name(iface) => (Some(true), Some(iface.to_string())),
        Some(_) => (Some(false), None),
        None => (None, None),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IPv6 availability
// ─────────────────────────────────────────────────────────────────────────────

fn detect_ipv6_available() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/net/ipv6_route") {
            Ok(content) => Some(proc_ipv6_route_has_default(&content)),
            Err(_) => None,
        }
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("route")
            .args(["-n", "get", "-inet6", "default"])
            .output()
            .ok()?;
        let (iface, gateway) = parse_route_get_output(&String::from_utf8_lossy(&out.stdout));
        Some(out.status.success() && (iface.is_some() || gateway.is_some()))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// True when Linux `/proc/net/ipv6_route` contains a `::/0` default route
/// (destination all-zeros with prefix length `00`).
pub fn proc_ipv6_route_has_default(content: &str) -> bool {
    content.lines().any(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        fields.len() >= 10
            && fields[0] == "00000000000000000000000000000000"
            && fields[1] == "00"
            && fields[9] != "lo"
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── /proc/net/route (Linux) ──────────────────────────────────────────────

    const PROC_NET_ROUTE: &str = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
eth0\t00000000\t0102A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
eth0\t0002A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0
docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0
";

    #[test]
    fn parses_linux_default_route_and_gateway() {
        let (iface, gw) = parse_proc_net_route(PROC_NET_ROUTE);
        assert_eq!(iface.as_deref(), Some("eth0"));
        // 0102A8C0 little-endian → 192.168.2.1
        assert_eq!(gw.as_deref(), Some("192.168.2.1"));
    }

    #[test]
    fn linux_default_route_prefers_lowest_metric() {
        let content = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlan0\t00000000\t0102A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
";
        let (iface, gw) = parse_proc_net_route(content);
        assert_eq!(iface.as_deref(), Some("eth0"));
        assert_eq!(gw.as_deref(), Some("192.168.1.1"));
    }

    #[test]
    fn linux_route_without_default_yields_none() {
        let content = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
eth0\t0002A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0
";
        assert_eq!(parse_proc_net_route(content), (None, None));
        assert_eq!(parse_proc_net_route(""), (None, None));
    }

    // ── route -n get default (macOS/BSD) ─────────────────────────────────────

    const ROUTE_GET_DEFAULT: &str = "\
   route to: default
destination: default
       mask: default
    gateway: 192.168.1.1
  interface: en0
      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>
 recvpipe  sendpipe  ssthresh  rtt,msec    rttvar  hopcount      mtu     expire
       0         0         0         0         0         0      1500         0
";

    #[test]
    fn parses_macos_route_get_output() {
        let (iface, gw) = parse_route_get_output(ROUTE_GET_DEFAULT);
        assert_eq!(iface.as_deref(), Some("en0"));
        assert_eq!(gw.as_deref(), Some("192.168.1.1"));
    }

    #[test]
    fn macos_link_level_gateway_is_ignored() {
        let output = "   route to: default\n    gateway: link#22\n  interface: utun4\n";
        let (iface, gw) = parse_route_get_output(output);
        assert_eq!(iface.as_deref(), Some("utun4"));
        assert_eq!(gw, None);
    }

    #[test]
    fn empty_route_get_output_yields_none() {
        assert_eq!(parse_route_get_output(""), (None, None));
    }

    // ── route print (Windows) ────────────────────────────────────────────────

    const ROUTE_PRINT: &str = "\
===========================================================================
Active Routes:
Network Destination        Netmask          Gateway       Interface  Metric
          0.0.0.0          0.0.0.0      192.168.1.1     192.168.1.23     25
===========================================================================
";

    #[test]
    fn parses_windows_route_print_gateway() {
        assert_eq!(
            parse_route_print_gateway(ROUTE_PRINT).as_deref(),
            Some("192.168.1.1")
        );
    }

    #[test]
    fn windows_on_link_default_is_skipped() {
        let output = "          0.0.0.0          0.0.0.0          On-link      10.0.0.5    281\n";
        assert_eq!(parse_route_print_gateway(output), None);
    }

    // ── networksetup -listallhardwareports (macOS) ───────────────────────────

    const HARDWARE_PORTS: &str = "\
Hardware Port: Wi-Fi
Device: en0
Ethernet Address: aa:bb:cc:dd:ee:ff

Hardware Port: Thunderbolt Ethernet
Device: en5
Ethernet Address: 11:22:33:44:55:66

Hardware Port: Thunderbolt Bridge
Device: bridge0
Ethernet Address: 99:88:77:66:55:44
";

    #[test]
    fn classifies_hardware_ports() {
        assert_eq!(classify_hardware_port(HARDWARE_PORTS, "en0"), "wifi");
        assert_eq!(classify_hardware_port(HARDWARE_PORTS, "en5"), "ethernet");
        assert_eq!(classify_hardware_port(HARDWARE_PORTS, "bridge0"), "virtual");
        assert_eq!(classify_hardware_port(HARDWARE_PORTS, "en99"), "unknown");
    }

    // ── ifconfig mtu (macOS/BSD) ─────────────────────────────────────────────

    #[test]
    fn parses_ifconfig_mtu() {
        let output = "en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\
                      \toptions=6460<TSO4,TSO6,CHANNEL_IO>\n";
        assert_eq!(parse_ifconfig_mtu(output), Some(1500));
        assert_eq!(parse_ifconfig_mtu("junk with no mtu keyword"), None);
    }

    // ── ipv6 default route (Linux) ───────────────────────────────────────────

    #[test]
    fn detects_linux_ipv6_default_route() {
        let with_default = "\
00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000064 00000000 00000000 00000003     eth0
";
        let without_default = "\
fe800000000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001     eth0
00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200       lo
";
        assert!(proc_ipv6_route_has_default(with_default));
        assert!(!proc_ipv6_route_has_default(without_default));
        assert!(!proc_ipv6_route_has_default(""));
    }

    // ── name-based classification ────────────────────────────────────────────

    #[test]
    fn vpn_and_virtual_name_heuristics() {
        for name in ["utun4", "tun0", "wg0", "tailscale0", "ppp0", "ipsec0"] {
            assert!(is_vpn_like_name(name), "{name} should look like a VPN");
            assert!(is_virtual_name(name), "{name} should look virtual");
        }
        for name in ["en0", "eth0", "wlan0", "enp3s0"] {
            assert!(!is_vpn_like_name(name), "{name} should not look like a VPN");
            assert!(!is_virtual_name(name), "{name} should not look virtual");
        }
        // Virtual but NOT VPN — the heuristic must stay conservative.
        for name in ["docker0", "br-42", "veth1a2b", "bridge0", "lo"] {
            assert!(is_virtual_name(name), "{name} should look virtual");
            assert!(!is_vpn_like_name(name), "{name} must not be flagged as VPN");
        }
    }

    // ── end-to-end collection smoke test ─────────────────────────────────────

    #[test]
    fn collect_against_loopback_does_not_panic_and_egress_ip_is_loopback() {
        let ctx = NetworkContext::collect("127.0.0.1", 80);
        // Egress toward loopback must be loopback, and must parse as an IP.
        let ip = ctx
            .local_ip
            .as_deref()
            .expect("local_ip should be collectable toward 127.0.0.1");
        let parsed: IpAddr = ip.parse().expect("local_ip must be a valid IP literal");
        assert!(parsed.is_loopback());
        // Kind, when present, is one of the documented values.
        if let Some(kind) = ctx.interface_kind.as_deref() {
            assert!(matches!(kind, "ethernet" | "wifi" | "virtual" | "unknown"));
        }
        // The struct round-trips through the JSON contract.
        let json = serde_json::to_string(&ctx).expect("serialize NetworkContext");
        let back: NetworkContext = serde_json::from_str(&json).expect("deserialize NetworkContext");
        assert_eq!(back, ctx);
    }

    #[test]
    fn collect_with_unresolvable_target_still_returns() {
        let ctx = NetworkContext::collect("host.invalid.networker.test", 443);
        // local_ip cannot be determined, but nothing panics and the rest of
        // the fields are still best-effort collected.
        assert!(ctx.local_ip.is_none());
    }

    #[test]
    fn empty_context_reports_empty() {
        assert!(NetworkContext::default().is_empty());
        let ctx = NetworkContext {
            mtu: Some(1500),
            ..Default::default()
        };
        assert!(!ctx.is_empty());
    }
}
