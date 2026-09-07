//! Kernel-WireGuard L3 carrier for `serve-tun --wireguard` (ADR-0001 data plane).
//!
//! filament owns identity, authentication and reachability; WireGuard moves the
//! bytes. The flow: filament first establishes its normal authenticated
//! (channel-bound) connection, then each side generates a Curve25519 WireGuard
//! keypair and swaps {public key, WG listen-port} over that authenticated stream.
//! With the peer's key and endpoint in hand, a kernel WireGuard interface carries
//! the L3 traffic at kernel speed, instead of the userspace QUIC-datagram pump.
//!
//! Linux-only, and requires the `wireguard` kernel module plus the `wg` and `ip`
//! tools (same iproute2 dependency the kernel TUN backend already relies on).

use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

/// Generate a WireGuard keypair, returned as (private_b64, public_b64), via `wg`.
/// Can this machine actually run kernel WireGuard right now?
///
/// A RUNTIME check, not a compile-time one. The platform surface gate keeps
/// per-platform branching out of files like this, and it would be the wrong tool
/// anyway: a Linux box without wireguard-tools, without the module, or without
/// CAP_NET_ADMIN cannot do this either. Asking the system is the only answer
/// that is true on the machine it runs on.
///
/// Deliberately quiet and cheap: it creates and immediately removes a probe
/// interface, which is the only way to learn whether the module and the
/// capability are both present without waiting for a real failure mid-session.
/// The interface filament manages. One device carries every peer, as WireGuard
/// intends: peers are distinguished by public key and allowed-ips, not by device.
pub const WG_DEV: &str = "filament-wg";

/// Linux refuses an interface name longer than this, and does it with a message
/// that names neither the length nor the field.
const MAX_IFNAME: usize = 15;

pub fn usable() -> bool {
    debug_assert!(WG_DEV.len() <= MAX_IFNAME, "wg device name too long for Linux");
    if Command::new("wg").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        return false;
    }
    // SHORT ON PURPOSE. Linux caps an interface name at 15 characters, and
    // "filament-wgprobe" is 16, so the probe failed with "Attribute failed
    // policy validation" on every machine and usable() always returned false.
    // That silently disabled the entire feature: no error, no log, just a
    // capability check that could never say yes. The device itself,
    // "filament-wg", is 11 and was always fine, which is why this only showed up
    // when the probe was added.
    const PROBE: &str = "fil-wgprobe";
    let _ = ip(&["link", "del", PROBE]);
    if ip(&["link", "add", PROBE, "type", "wireguard"]).is_err() {
        return false;
    }
    let _ = ip(&["link", "del", PROBE]);
    true
}

pub fn gen_keypair() -> Result<(String, String)> {
    let out = Command::new("wg")
        .arg("genkey")
        .output()
        .context("run `wg genkey` (is wireguard-tools installed?)")?;
    if !out.status.success() {
        bail!("wg genkey failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let privkey = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let mut child = Command::new("wg")
        .arg("pubkey")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("spawn `wg pubkey`")?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("wg pubkey: no stdin"))?
        .write_all(format!("{privkey}\n").as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("wg pubkey failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let pubkey = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((privkey, pubkey))
}

fn ip(args: &[&str]) -> Result<()> {
    let out = Command::new("ip").args(args).output().context("exec ip")?;
    if !out.status.success() {
        bail!("ip {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Create the WG interface with our private key and an ephemeral listen-port, and
/// return the port the kernel actually chose (advertised to the peer as our endpoint).
pub fn create_iface(dev: &str, privkey: &str) -> Result<u16> {
    let _ = ip(&["link", "del", dev]); // best-effort clear a stale interface
    ip(&["link", "add", dev, "type", "wireguard"]).with_context(|| format!("create wg dev {dev} (is the wireguard module loaded?)"))?;

    // `wg set <dev> private-key <path>`: feed the key on stdin via /dev/stdin so it
    // never lands on disk.
    let mut child = Command::new("wg")
        .args(["set", dev, "private-key", "/dev/stdin", "listen-port", "0"])
        .stdin(Stdio::piped())
        .spawn()
        .context("spawn `wg set private-key`")?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("wg set: no stdin"))?
        .write_all(format!("{privkey}\n").as_bytes())?;
    if !child.wait()?.success() {
        let _ = ip(&["link", "del", dev]);
        bail!("wg set private-key failed");
    }

    // Bring the interface up so `wg show listen-port` returns the ephemeral
    // port the kernel actually assigned (down interfaces always report 0).
    ip(&["link", "set", "dev", dev, "up"]).with_context(|| format!("bring {dev} up"))?;

    let out = Command::new("wg").args(["show", dev, "listen-port"]).output().context("wg show listen-port")?;
    if !out.status.success() {
        bail!("wg show listen-port failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    String::from_utf8_lossy(&out.stdout).trim().parse::<u16>().context("parse wg listen-port")
}

/// Attach the peer (key + endpoint + allowed-ips), address the interface, bring it up.
pub fn configure_peer(
    dev: &str,
    peer_pub: &str,
    allowed_ips: &str,
    endpoint: &str,
    addr_cidr: &str,
    mtu: u32,
) -> Result<()> {
    let out = Command::new("wg")
        .args([
            "set", dev, "peer", peer_pub, "allowed-ips", allowed_ips, "endpoint", endpoint,
            "persistent-keepalive", "25",
        ])
        .output()
        .context("wg set peer")?;
    if !out.status.success() {
        bail!("wg set peer: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    // Idempotent: the address belongs to the INTERFACE, not to a peer, so the
    // second peer would otherwise fail here with "File exists" and take the
    // whole establish down with it.
    if let Err(e) = ip(&["addr", "add", addr_cidr, "dev", dev]) {
        if !e.to_string().contains("File exists") {
            return Err(e).context("ip addr add on wg dev");
        }
    }
    ip(&["link", "set", "dev", dev, "mtu", &mtu.to_string()]).context("ip link set mtu")?;
    ip(&["link", "set", "dev", dev, "up"]).context("ip link set up")?;
    Ok(())
}

/// Remove the WG interface (best-effort; used on teardown).
pub fn teardown(dev: &str) {
    let _ = ip(&["link", "del", dev]);
}

/// Our half of the exchange: make sure the interface exists and report what the
/// peer needs to know about us. Symmetric, so there is no initiator.
pub fn local_offer() -> Result<(String, u16)> {
    let (privkey, pubkey) = iface_identity()?;
    let port = match existing_listen_port() {
        Some(p) => p,
        None => {
            let p = create_iface(WG_DEV, &privkey)?;
            crate::ui::debug(&format!("  wg: created {WG_DEV} on port {p}"));
            p
        }
    };
    Ok((pubkey, port))
}

/// Configure the peer that just announced itself, and route its overlay address
/// down the tunnel.
pub async fn adopt_peer(
    peer_pub: &str,
    peer_overlay: &str,
    our_overlay: &str,
    mtu: u32,
    transport: std::sync::Arc<dyn crate::net::Transport>,
) -> Result<()> {
    let (_priv, _pub) = iface_identity()?;
    let wg_port = existing_listen_port().ok_or_else(|| anyhow!("no local WireGuard listen port"))?;

    // The peer's stand-in, on loopback. WireGuard talks only to this; filament
    // carries the frames over the path it has already punched, so NAT never sees
    // a WireGuard packet and WireGuard never has to traverse one.
    let relay = std::sync::Arc::new(Relay::bind(wg_port).await?);
    let relay_port = relay.local_port()?;
    let allowed = format!("{peer_overlay}/128");
    configure_peer(
        WG_DEV,
        peer_pub,
        &allowed,
        &format!("127.0.0.1:{relay_port}"),
        &format!("{our_overlay}/128"),
        mtu,
    )?;
    // Host-scoped, so a WireGuard peer never captures traffic for a peer that is
    // not on the tunnel.
    let _ = ip(&["route", "replace", &allowed, "dev", WG_DEV]);
    register_relay(peer_overlay, relay.clone());

    // OUTBOUND pump: whatever the kernel hands the stand-in goes to the peer.
    // Inbound is handled by the L3 datagram pump, which already reads every
    // datagram from this peer and now splits WireGuard frames off by first byte.
    let sock = relay.socket();
    let peer_label = peer_overlay.to_string();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let Ok((n, _from)) = sock.recv_from(&mut buf).await else { break };
            if transport.send_datagram(&buf[..n]).is_err() {
                break; // link gone; the reconcile will rebuild on the next one
            }
        }
        crate::ui::debug(&format!("  wg: relay pump for {peer_label} ended"));
    });

    crate::ui::debug(&format!(
        "  wg: peer {peer_overlay} routed through the local relay on 127.0.0.1:{relay_port}"
    ));
    Ok(())
}

/// This interface's keypair, generated once per process.
///
/// Cached because the interface has exactly one identity: see `establish` for
/// what regenerating it per peer breaks.
static WG_IDENTITY: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

fn iface_identity() -> Result<(String, String)> {
    if let Some(kp) = WG_IDENTITY.get() {
        return Ok(kp.clone());
    }
    let kp = gen_keypair()?;
    // A race here would mean two callers generated keys; the first one stored
    // wins and both return it, so the interface and what we advertise agree.
    let _ = WG_IDENTITY.set(kp);
    Ok(WG_IDENTITY.get().cloned().expect("just set"))
}

/// The listen port of an interface we already created, if any.
fn existing_listen_port() -> Option<u16> {
    let out = Command::new("wg").args(["show", WG_DEV, "listen-port"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Peers this process has a tunnel to, or is currently building one for.
///
/// The reconcile runs on a timer, so without this a slow rendezvous would be
/// re-attempted every tick and the two ends would pile up half-open bi-streams
/// against each other. Claim before spawning, release on failure so the next
/// tick retries, keep it on success.
static WG_ATTEMPTED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn attempted() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    WG_ATTEMPTED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// True if this call took the claim for `peer`; false if someone already has it.
pub fn claim_attempt(peer: &str) -> bool {
    attempted()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(peer.to_string())
}

/// Give the claim back so a later tick can retry.
pub fn release_attempt(peer: &str) {
    attempted().lock().unwrap_or_else(|e| e.into_inner()).remove(peer);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe name being one character over the Linux limit made usable()
    /// return false on every machine, which disabled WireGuard silently: no
    /// error, no log, just a capability check that could never say yes.
    #[test]
    fn interface_names_fit_within_the_kernel_limit() {
        assert!(WG_DEV.len() <= MAX_IFNAME, "{WG_DEV} is {} chars", WG_DEV.len());
        assert!(
            "fil-wgprobe".len() <= MAX_IFNAME,
            "the probe name must fit too, or usable() silently answers no"
        );
    }
}

// ── WireGuard over filament's transport ──────────────────────────────────────
//
// WHY. Kernel WireGuard owns its own UDP socket, so it does its own NAT
// traversal, and it has none: both daemons announced their INTERNAL listen port,
// the NAT had no inbound mapping, and the handshake was dropped. That is decision
// 2 of the ADR being violated, and the fix is not to teach WireGuard about NAT.
//
// Instead WireGuard never touches the network. Each side points its peer's
// endpoint at a filament-owned UDP socket on LOOPBACK, and filament carries the
// frames over the path it has already punched:
//
//   kernel WG --UDP--> 127.0.0.1:relay --filament transport--> peer's relay --> its WG
//
// The demux needs no format change. Datagrams already carry raw IP packets and
// the receiver reads the version nibble; a WireGuard frame's first byte is its
// message type, 1..=4, which cannot collide with 0x4_ (IPv4) or 0x6_ (IPv6). So a
// WireGuard frame is self-identifying on the existing datagram path.
//
// This is not slower than the plane it joins: the hot path was
// `TUN read -> transport` and is now `UDP read -> transport`, the same number of
// userspace copies, with the crypto moved into the kernel.

/// Is this datagram a WireGuard frame rather than an IP packet?
///
/// WireGuard message types are 1 (handshake init), 2 (response), 3 (cookie) and
/// 4 (transport data). IPv4 starts 0x4_, IPv6 0x6_. The ranges cannot overlap,
/// which is what lets both share one datagram channel untagged.
pub fn is_wg_frame(pkt: &[u8]) -> bool {
    matches!(pkt.first(), Some(1..=4))
}

/// A loopback UDP socket that stands in for the peer, as far as kernel
/// WireGuard is concerned.
pub struct Relay {
    sock: std::sync::Arc<tokio::net::UdpSocket>,
    /// Where the LOCAL WireGuard is listening, so inbound frames can be handed to it.
    wg_port: u16,
}

impl Relay {
    /// Bind on loopback only. This socket must never be reachable from the
    /// network: it is the peer's stand-in, and anything that could reach it
    /// could inject frames the peer never sent.
    pub async fn bind(wg_port: u16) -> Result<Self> {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .context("bind the WireGuard relay socket on loopback")?;
        Ok(Self { sock: std::sync::Arc::new(sock), wg_port })
    }

    /// The port to give WireGuard as its peer's endpoint.
    pub fn local_port(&self) -> Result<u16> {
        Ok(self.sock.local_addr().context("relay local addr")?.port())
    }

    pub fn socket(&self) -> std::sync::Arc<tokio::net::UdpSocket> {
        self.sock.clone()
    }

    /// Hand an inbound frame to the local WireGuard.
    ///
    /// Sent FROM this socket, so WireGuard sees it arriving from the endpoint it
    /// was configured with and replies here rather than to the network.
    pub async fn deliver_to_wg(&self, pkt: &[u8]) -> Result<()> {
        self.sock
            .send_to(pkt, ("127.0.0.1", self.wg_port))
            .await
            .map(|_| ())
            .context("deliver a WireGuard frame to the local interface")
    }
}

/// Relays by peer overlay address, so the datagram pump can find the one that
/// belongs to the peer a frame arrived from.
static RELAYS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Relay>>>,
> = std::sync::OnceLock::new();

fn relays() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<Relay>>> {
    RELAYS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub fn register_relay(peer_overlay: &str, relay: std::sync::Arc<Relay>) {
    relays().lock().unwrap_or_else(|e| e.into_inner()).insert(peer_overlay.to_string(), relay);
}

pub fn relay_for(peer_overlay: &str) -> Option<std::sync::Arc<Relay>> {
    relays().lock().unwrap_or_else(|e| e.into_inner()).get(peer_overlay).cloned()
}

#[cfg(test)]
mod relay_tests {
    use super::*;

    /// The whole demux rests on these ranges never overlapping.
    #[test]
    fn wireguard_frames_and_ip_packets_are_distinguishable() {
        for t in 1u8..=4 {
            assert!(is_wg_frame(&[t, 0, 0, 0]), "message type {t} is WireGuard");
        }
        assert!(!is_wg_frame(&[0x45, 0, 0, 0]), "IPv4 header");
        assert!(!is_wg_frame(&[0x60, 0, 0, 0]), "IPv6 header");
        assert!(!is_wg_frame(&[]), "an empty datagram is not a WireGuard frame");
    }
}
