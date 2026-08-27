//! Fleet auto-mesh: devices certified by ONE owner key meet on a single
//! rendezvous channel and admit each other on that certificate, never on
//! presence.
//!
//! Design: `docs/design-fleet-automesh.md`. Model check:
//! `proofs/fleet_automesh_model.py` (gated in CI by `.github/workflows/proof.yml`).
//!
//! The channel is a MEETING POINT and nothing more. Anyone who learns its id can
//! see that devices are present; they cannot be admitted, because admission
//! requires an owner-signed `DeviceCert` bound to a live possession proof over
//! this link's channel binding. The model checker's Intruder tier exists to keep
//! that claim honest: it parks an uncertified device on every channel id it
//! could learn and asserts nobody admits it.
//!
//! What auto-mesh grants is REACHABILITY, not capability. A newly met sibling is
//! admitted with an empty ceiling: the link forms, warm-hold keeps it, L3 routes
//! to it. Every capability (transfer, shell, mount) still needs its own explicit
//! grant, exactly as before. So a bug in this file cannot escalate privilege, it
//! can only connect something it should not have connected.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::identity::DeviceCert;
use crate::overlay::Announce;

/// The `type` of the mutual fleet handshake control message.
pub const HELLO: &str = "fleet-hello";

fn rv_path() -> std::path::PathBuf {
    crate::platform::Paths::config_path("fleet.rv")
}

/// The fleet rendezvous secret this device holds, if any (64 hex chars).
pub fn rv() -> Option<String> {
    let raw = std::fs::read_to_string(rv_path()).ok()?;
    let s = raw.trim().to_string();
    (hex::decode(&s).map(|b| b.len()).ok() == Some(32)).then_some(s)
}

/// The owner's copy, minted on first use. Only a device holding the UserKey
/// calls this; every other device RECEIVES the secret at enrollment.
pub fn rv_load_or_create() -> Result<String> {
    if let Some(s) = rv() {
        return Ok(s);
    }
    let s = crate::fresh_secret();
    store_rv(&s)?;
    Ok(s)
}

/// Persist a fleet rendezvous secret received at enrollment.
pub fn store_rv(hex_secret: &str) -> Result<()> {
    if hex::decode(hex_secret).map(|b| b.len()).ok() != Some(32) {
        bail!("fleet rendezvous secret must be 32 bytes of hex");
    }
    crate::platform::SecretFile::write_str(&rv_path(), hex_secret)?;
    Ok(())
}

/// The fleet meeting point. Deliberately the SAME construction as a pair channel
/// (`sha256("filament-pair:" || secret)`), so the signaling server validates it
/// with the existing `CHAN_RE`, stores nothing new, and gains no new power.
pub fn channel() -> Option<String> {
    rv().map(|s| crate::channel_of(&s))
}

/// True when `ch` is our fleet channel. Cheap enough for the presence path.
pub fn is_fleet_channel(ch: &str) -> bool {
    !ch.is_empty() && channel().as_deref() == Some(ch)
}

/// Build our `fleet-hello` for a link whose channel binding is `cb`.
///
/// Reuses `overlay::Announce` rather than inventing a second possession proof:
/// enrollment already forces the overlay key and the certified device key to be
/// the SAME key (`fleet_enrollment::ensure_same_device_key`), so an announce
/// that verifies against `cb` is already a live possession proof of the key the
/// certificate names.
pub fn make_hello(cb: &[u8], name: &str) -> Result<Value> {
    let id = crate::overlay::Identity::load_or_create()?;
    let ann = id.announce(crate::overlay::next_announce_seq(), cb);
    let cert = crate::local_device_cert()
        .ok_or_else(|| anyhow!("this device holds no certificate to present"))?;
    Ok(json!({
        "type": HELLO,
        "name": name,
        "cert": cert.to_json(),
        "announce": ann.to_json(),
    }))
}

/// What a verified `fleet-hello` establishes about the peer.
#[derive(Debug, Clone)]
pub struct Verified {
    pub device_pub: [u8; 32],
    pub owner_pub: [u8; 32],
    pub cert_expires: u64,
    pub claimed_name: String,
}

/// Verify a `fleet-hello` against OUR owner key and THIS link's channel binding.
///
/// Order matters for what an error MEANS, not for what passes: all three checks
/// are required.
pub fn verify_hello(
    v: &Value,
    cb: &[u8],
    my_owner_pub: &[u8; 32],
    now: u64,
) -> Result<Verified> {
    let ann = Announce::from_json(&v["announce"])?;
    // Possession of the announced key, bound to THIS link. Without the binding a
    // hello captured on one link would replay onto another.
    ann.verify(cb)?;
    let cert = DeviceCert::from_json(&v["cert"])
        .ok_or_else(|| anyhow!("fleet-hello carried no certificate"))?;
    let name = v["name"].as_str().unwrap_or("peer").to_string();
    bind_cert_to_possession(&cert, ann.pubkey, my_owner_pub, now, name)
}

/// The non-crypto half, split out so the binding rule is unit-testable without
/// forging signatures over a channel binding.
fn bind_cert_to_possession(
    cert: &DeviceCert,
    proven_pubkey: [u8; 32],
    my_owner_pub: &[u8; 32],
    now: u64,
    claimed_name: String,
) -> Result<Verified> {
    // The certificate must name the key that was just PROVEN on this link.
    // Without this, a valid certificate for device X could front for device Y's
    // possession proof: two halves that are each fine and together are a lie.
    crate::fleet_enrollment::ensure_same_device_key(cert.device_pub, proven_pubkey)?;
    // ... and it must chain to the owner key WE already hold. This is the step
    // that makes admission non-transitive: no peer's assertion is an input.
    cert.verify_chain(my_owner_pub, now)?;
    Ok(Verified {
        device_pub: cert.device_pub,
        owner_pub: cert.user_pub,
        cert_expires: cert.expires,
        claimed_name,
    })
}

/// Our own owner key, as every fleet check needs it. The owner device holds the
/// UserKey; a joined device carries the owner's key inside its own certificate.
pub fn my_owner_pub() -> Option<[u8; 32]> {
    if let Ok(Some(uk)) = crate::identity::UserKey::load(&crate::platform::PlatformKeyStore) {
        return Some(uk.public_key_bytes());
    }
    crate::local_device_cert().map(|c| c.user_pub)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cert(device_pub: [u8; 32], user_pub: [u8; 32]) -> DeviceCert {
        DeviceCert { device_pub, user_pub, expires: u64::MAX, issued: 0, sig: [0; 64] }
    }

    #[test]
    fn rejects_cert_that_does_not_name_the_proven_key() {
        // A perfectly valid certificate for device [1;32], presented on a link
        // where device [2;32] proved possession. Must be refused BEFORE the
        // signature is even considered, so a real cert cannot front for another
        // device's proof.
        let err = bind_cert_to_possession(
            &cert([1; 32], [9; 32]), [2; 32], &[9; 32], 1_000, "peer".into(),
        ).unwrap_err();
        assert!(err.to_string().contains("key mismatch"), "got: {err}");
    }

    #[test]
    fn rejects_cert_chaining_to_another_owner() {
        // Possession is genuine and the cert names the right device, but it was
        // signed by somebody else's owner key. This is the non-transitivity
        // check: another fleet's certificate is not evidence in ours.
        let err = bind_cert_to_possession(
            &cert([1; 32], [8; 32]), [1; 32], &[9; 32], 1_000, "peer".into(),
        ).unwrap_err();
        assert!(err.to_string().contains("different user key"), "got: {err}");
    }

    #[test]
    fn store_rv_rejects_a_wrong_length_secret() {
        assert!(store_rv("abcd").is_err());
        assert!(store_rv("zz".repeat(32).as_str()).is_err());
    }

    #[test]
    fn is_fleet_channel_is_false_for_empty() {
        assert!(!is_fleet_channel(""));
    }
}
