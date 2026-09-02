//! Exit nodes: accepting a DEFAULT route without cutting your own underlay.
//!
//! A subnet route and a default route look identical in the protocol and are
//! completely different in the kernel. `10.66.0.0/24 dev filament0` is additive.
//! `0.0.0.0/0 dev filament0` is not: it captures every packet the machine sends,
//! including the ones carrying the overlay itself. Install it naively into the
//! main table and the tunnel's own traffic is routed into the tunnel, the link
//! dies, the route is withdrawn, the link returns, and the machine oscillates.
//! The first packet lost is usually the one to the signaling server, so the node
//! also cannot be told to stop.
//!
//! Accepting one safely needs policy routing: a separate table, a rule pointing
//! at it, and carve-outs for the peer's own underlay address and the signaling
//! server. The full plan, including the ordering constraints that make a
//! half-applied or half-removed state safe, is written up in
//! `docs/design-subnet-routes.md` under "Exit nodes".
//!
//! Only the DETECTION lives here, because it is the only part with a caller
//! today. The planner is deliberately not carried in the tree as dead code: the
//! artifact registry's rule is that the unwired set may only shrink, and this
//! repository has already been burned once by a module that was declared,
//! never called, and described in the roadmap as ready (see the WireGuard L3
//! entry in the 2026-08-28 build-order audit).
//!
//! WHAT BLOCKS THE REST: the carve-out for the peer needs its underlay address,
//! and `net::Transport` exposes no endpoint accessor. Adding one is the next
//! step. Advertising a default route already works end to end; it is accepting
//! one that waits.

use std::net::IpAddr;

/// Is this prefix a default route, the thing that cannot go in the main table?
pub fn is_default_route(net: IpAddr, len: u8) -> bool {
    len == 0 && net.is_unspecified()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_a_default_route_in_both_families() {
        assert!(is_default_route("0.0.0.0".parse().unwrap(), 0));
        assert!(is_default_route("::".parse().unwrap(), 0));
        assert!(!is_default_route("10.66.0.0".parse().unwrap(), 24));
        // A /0 that is not the unspecified address is malformed, not a default.
        assert!(!is_default_route("10.0.0.1".parse().unwrap(), 0));
    }
}
