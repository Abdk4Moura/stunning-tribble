# Work state and task list

> Living index of where filament + the product stand, written so context survives
> across sessions and compaction. The committed design docs are the source of
> truth; this file is the map and the ordering. Update status here as work lands.
> Last updated: 2026-08-25.

## One-paragraph state

filament's L3 data plane is moving to WireGuard (built + measured, branch
`feat/wireguard-l3`, ready to merge). The identity/access/product design is
complete and committed. The trust root, L1 PAKE v2, is spec'd with owner decisions
locked (`L1-pake-protocol.md` §13) and is the next thing to build, contracted to
filament-professional. Product thesis: a no-account pairwise mesh plus a secure
compute-pooling platform on top; the p2p send is the consumer wedge; enterprise
features are a demand-pulled compliance envelope built LAST.

## The core / product boundary (READ FIRST)

filament is the CORE/platform: pairing (L1), the transport ladder (L2), the overlay
(L3 = WireGuard), identity, capabilities, transfer, and `mount`, exposed through a
stable PRODUCT INTERFACE (the local control socket, `ctl.rs`). PRODUCTS -- the
compute/GPU product and any others -- are SEPARATE codebases that CONSUME that
interface. Nothing product-specific (GPU sandbox, job protocol, OpenAI routing)
lives in filament core. filament's job is to make itself usable for those purposes,
not to be them.

**NO BACKWARD COMPAT (2026-07-26).** No real users yet (owner + friends), so clean
breaks everywhere: no v1 paths, no fallback shims, no mixed-version negotiation, no
migration windows. Prioritize clean single-path implementations; everyone re-pairs
/ reconnects on the current format. (Applies to L1 gate #7 and the L2 per-sid v1
fallback, both dropped.)

## Master build order (and why this sequence)

THE TRUST ARC (sequential, one continuous code area, filament-professional):
1. **L1 PAKE v2** (trust root). PAKE closes the first-pairing MITM gap; produces
   the per-device pinned secret + v2 device record. IN PROGRESS.
2. **Identity layer** (user-key over device-certs). The CONTINUATION of L1: extends
   the same pairing/device-record code; ADDITIVE on top of L1's device secret (user
   key vouches for device certs). SSH-CA shaped. filament-professional continues here.
3. **Capabilities / grant** (edge-local typed signed ops). Device-certs carry caps;
   generalizes L2's `filament grant`; the exposed authz primitive; meets the product
   interface at `open`.

PARALLEL CORE TRACKS (independent of the trust arc):
4. **L2 per-sid QUIC streams** (HoL-free multiplexing). Each sid its own QUIC
   stream. IN PROGRESS -> mimo-0x0 (`design-l2-perstream-quic.md`, single-path).
5. **Merge WireGuard L3 data plane** (`feat/wireguard-l3`). Built + measured, ready.
6. **Product interface** (`docs/design-product-interface.md`). Formalize `ctl.rs`
   into a stable local API + SDK. Exposes whatever identity/grant model exists, so
   it proceeds alongside the trust arc. Highest-leverage "make filament usable" move.

PRODUCTS (downstream, SEPARATE consumers of the interface):
7. **Compute product (lend-gpu).** GPU sandbox + consent + edge routing
   (OpenAI-compatible) + model distribution (`mount <cid>`). The p2p-send inverted.
   NOT filament core.
8. **Enterprise envelope** (org authority: SSO/audit/device-approval). LAST,
   demand-pulled, not a wedge.

REACHABILITY (a filament-core concern surfaced 2026-07-26, to spec): a public-IP
filament node doubles as the user's RENDEZVOUS + RELAY (not a coordinator; moves
ciphertext + brokers hole-punch, never authorizes). Multiple public nodes = an
interchangeable endpoint SET published in the DHT (introduce-gated, on top of the
pairwise core), TTL'd so a dead node drops and devices happy-eyeballs a live one
(failover without leader election). api.filament = opt-out-able hosted fallback.
Aligned with `design-mesh-network.md` (relay-peer doubles as rendezvous,
self-hostable, distinct services).

Fabric is the FOUNDATION we own standalone (Tailscale = compat bridge only). The
WEDGE is compute (govern USE, not reachability), built as a separate product ON the
core. Pitch: "secure compute pooling among trusted devices," never "a no-account
Tailscale."

## Status by workstream

### WireGuard L3 data plane -- BUILT + MEASURED, ready to merge
- ADR: `docs/adr-0001-wireguard-as-l3-data-plane.md`
- Branch `feat/wireguard-l3`: adr `6639d6d`, wg module `691a4be`, wiring `08f72a4`.
- Numbers: loopback 0.48 -> 1.37 Gbps (0 retransmits vs 382); cross-machine
  do-vm<->other-do (~2 Gbps UDP policer) 396 -> 1256 Mbps (~99% of the UDP ceiling,
  near-clean). WG is env-bound here (shared CPU / policer), not its own ceiling.
- `serve-tun --wireguard`: filament brokers WG keys over its authed conn; kernel WG
  moves bytes. `cli/src/wg.rs` + `main.rs` wiring.
- NEXT: final review + rebase on main + merge. Bench scripts in the job tmp
  (`l3bench.sh`, `wgbench.sh`, `cross_wg_bench.sh`).

### L1 PAKE v2 (trust root) -- SPEC'D, DECISIONS LOCKED, to build
- Spec: `docs/L1-pake-protocol.md` (decisions §13, 2026-07-26).
- Locked: v2-only cutover (no v1 fallback); auto-mint 16-bit (256x256);
  user-chosen gate = "2 distinct words, not a common phrase"; gate #3
  (burn-on-claim + no-retry) HARD + negatively tested; nameplate 900 + monitor;
  `spake2 = "=0.4.0"`.
- Browser PAKE v2 substantially built (`frontend/src/lib/words.js`, `pairing.js`,
  `PakePairing`, custom-code form). Remaining: CLI/native side, server
  nameplate-only split (remove word minting in `backend/signaling.py`), decisions
  2-4, and the §10 gates (esp. negative gates #2 server-can't-derive, #3 burn+no-retry).
- NEXT: contract to filament-professional.

### L2 per-sid QUIC streams -- IN PROGRESS (mimo-0x0)
- Spec: `docs/design-l2-perstream-quic.md` (PROPOSED). Each L2 `sid` gets its own
  QUIC bidi stream (QUIC gives per-stream flow control + no cross-stream HoL), so a
  stalled stream no longer blocks the others. App-credit (`feat/l2-credit`) was
  tried and FAILED validation. SINGLE PATH (no v1 fallback / no compat needed).
- File: `cli/src/direct.rs` (also touched by the WireGuard branch; merge ordering TBD).
- NEXT: mimo-0x0 returns a plan, then implements; money test = fast stream not
  blocked by a stalled slow stream over one link.

### Product interface -- CORE, to build
- The seam: `ctl.rs`, the local 0600 unix control socket (JSON line + reply +
  raw bytes; ~15 internal ops today: open/dial/pty/mount/...). Used only by the CLI
  talking to its own daemon; internal and unstable.
- The work: (1) stabilize + version + document it as a public contract; (2) fill
  gaps a product needs -- identity/pair, peers + an EVENTS subscription, open a
  stream to peer:action gated by the grant model, transfer, grants, mount; (3) a
  thin SDK (Python first for the compute MVP, then Rust/JS).
- Boundary: filament owns the API + primitives; products own compute-specific logic
  on the consumer side. Capabilities meet the interface at `open`.
- Spec: `docs/design-product-interface.md` (design 2026-07-26, stress-tested):
  versioned JSON-NDJSON over UDS, persistent-subscription events (2 lanes,
  replay), grants = authz + registration = service-discovery/consent, 0600 now +
  scope-able later. Build seq ends by porting the lend-gpu MVP onto the SDK.

### Identity + access UX -- DESIGNED + COMMITTED
- `docs/design-identity-access-ux.md`: onboarding, contact book, introduce,
  recovery presets, capabilities (edge-local typed-signed-op CRDT), postures +
  enterprise-as-envelope positioning.
- `docs/design-introduce-user-identity.md`: user-key over device-certs (SSH-CA
  shaped), introduce-to-one-device, privacy of the device set.
- Recovery: 2 dials -> 4 presets (Anonymous-purist / Consumer-default /
  High-availability / Trusted-circle); blind federated timestamping witness for the
  compromise race; 7-day pending + old-key freeze; duress PIN; private per-contact
  contacts.

### GPU / compute product -- FEASIBILITY + POSITIONING COMMITTED
- `docs/gpu-product-feasibility.md`: trusted-group edge inference is the feasible
  wedge; open marketplace gated on verifiable-compute + open-DHT Sybil;
  identity-continuity is the owed design.
- lend-gpu MVP: `runner/` Python (23 tests): sandbox (`--network none`, mem/cpu
  caps) + consent gate + send/borrow. Awaiting GPU-host validation + Rust subcommand.

### Mesh / discovery -- DECIDED (filament stays pairwise)
- `docs/design-mesh-network.md`: no mesh/DHT/coordinator in filament core. Any
  product discovery rides on top; closed-by-default, introduce-gated (Sybil research
  conclusion: introduce IS the Sybil defense).

## Pointers
- Memory: `gpu-product-mesh-wireguard.md` (durable summary of all of the above).
- Portfolio (public): "The Data Plane Underneath", "Who are you, really".
- Adversarial design reviews this session ran through xats agent `wise-agent`.


## 2026-08-25: fleet auto-mesh, L3 default-on, L3 over relay

Branch `work/fleet-automesh`, uncommitted. Design:
`docs/design-fleet-automesh.md`, `docs/design-l3-over-relay.md`, and the
amendment section in `docs/design-mesh-network.md`. Proof:
`proofs/fleet_automesh_model.py`, gated in `.github/workflows/proof.yml`.

### Landed and verified on real daemons

- **Fleet auto-mesh.** Devices holding a DeviceCert from the SAME owner key
  discover each other on one rendezvous channel and admit on the certificate,
  never on presence. Verified with three daemons where B and C never exchanged a
  code or secret and each ended up seeing the other.
- **L3 on by default.** `tun-addr` defaults to `auto`; `filament init` asks for
  the one-time CAP_NET_ADMIN grant. Verified: a fresh install brings up
  `filament0` dual-stack with a working `<name>.mesh`.
- **L3 over relay.** IP packets ride a reserved sid on the DataChannel, and the
  announce is bound by a per-link nonce challenge because webrtc-rs exposes no
  RFC-5705 exporter. Verified: with the direct ladder refused, `ping6` across the
  overlay returns 3/3 over a DataChannel-only link.
- **`expected_secret` sweep.** All 44 sites audited; two were wrong (the
  in-session pairing ceremony could be consumed by a fleet peer, and
  `digest_says_alone` mis-counted fleet links).
- Incidental: pidfile compat fix (`e76d6d6` wrote two lines, this branch parsed
  one), which had made `status` blind to a running daemon and would have let `up`
  start a second one on the same config.

### Next, in order, with the obligation attached

1. **Fleet peers: indexed (DONE), addressable (OPEN).** A verified sibling is now
   written to `devices.json` and listed under FLEET with an empty ceiling, with
   NO pair secret, so it can never become a channel subscription or dial target.
   Two related fixes went with it: the listing iterated `devices_load()` (which
   requires a secret, hiding the very records it should show), and the FLEET tier
   asked `load_owner_key()`, "do I hold the owner's PRIVATE key", so every joined
   device rendered its own siblings as EXTERNAL.
   STILL OPEN: the L2 verbs cannot dial them. `send --to` now says so accurately
   rather than claiming the name is unknown while listing it. Making them
   addressable needs one-shot verbs to ride the daemon's warm link, since there is no secret to dial with.
   **RESOLVED (2026-08-26): the obligation is discharged.** The negative test has
   now been RUN and PASSED. With `laptop`'s shell acceptor armed for a different
   device, a fleet sibling with zero grants is denied by the gate itself:

       l2: pty refused: phone: not in auth key caps

   while `reach` over the same link succeeds. Reachability and capability are
   demonstrably separate. Three bugs had to be fixed first, each of which stopped
   the request BEFORE the gate and each of which presented as "peer unreachable":
   the warm path is skipped for non-tty stdio (so redirected test runs never
   asked the daemon); fleet dial GLARE (both ends dialing, 1 verify / 10 drops,
   never settling, fixed with a deterministic `conn.my_id < pid` tiebreak, after
   which it is 1 verify / 0 drops); and `warm_link_for` gating on `l.trusted`,
   which a fleet link deliberately never sets.

   **UX DEFECT: FIXED.** A refusal used to present as a reachability failure.
   Measured from a real terminal:

       before:  45s   ->  "can't reach 'laptop' ... it may be offline"
       after:   28ms  ->  "the peer closed the shell request (capability not granted?)"

   Four layers, each hiding an answer the peer had already given:
   1. `l2-open` and `l2-close` shared the `l2_enabled` guard, which answers "will
      I ACCEPT inbound opens". Right for an open, wrong for a close, since a
      close is the peer answering a stream WE opened. So the client discarded the
      refusal it had just asked for. Split, and this was the load-bearing one:
      the close now reaches `Mux::on_close`, which drops the stream, which ends
      `verify_first_frame` with "closed before any frame" instead of a timeout,
      which is what lets a client tell a REFUSAL from a dead link.
   2. The daemon treated EVERY warm-pty failure as a zombie, tearing down a
      healthy link on every refusal. Only silence inside the verify window is a
      zombie now.
   3. `ctl::try_pty` collapsed every rejection to a bare `None`, losing the
      reason. `try_pty_reason` carries it.
   4. The client fell through to a COLD retry on refusal, and that retry died at
      name resolution, so ITS error is what surfaced. A `refused:` reason is now
      definitive and stops there.

   The identical hole had already been found and fixed once for MOUNT: see the
   #206 comment on `Mux::on_close`, which records that the reason "was dropped
   here and the initiator read a generic channel-closed". The pty path had it one
   layer up and nobody had walked it.

   **RELEASE-VERIFIED (2026-08-26).** Release build succeeded, installed, and
   re-verified on the release binary: three-node mesh forms with B and C never
   paired (A sees laptop+phone, B sees owner+phone, C sees laptop+owner), and the
   deny path reports "the peer closed the shell request (capability not
   granted?)" in ~2s rather than 45s of "may be offline". 410/410 tests green.
   The shipped binary carries no test hooks.

   **(historical note)** Everything from the `verified_name` binding onward is
   type-checked and exercised with a DEBUG build (release builds were killed
   externally three times, not OOM). Release-build, run the full suite, and
   install before relying on any of it. A `fleet-hello decision ...` debug line
   was left in the ChannelReady path deliberately; it is what made the glare
   diagnosable and is debug-level only.

   **Obligation, unchanged:** that step promotes the empty-ceiling capability
   gate from a second line of defence into the load-bearing one. A fleet peer is
   still stopped earlier, at name resolution and link establishment, so the gate
   has never been exercised live. Re-run the negative test (a fleet peer
   attempting `shell` and `send --to` against a sibling that granted it nothing)
   as PART of that change, not after it.
1a. **PRIVILEGE ESCALATION FOUND AND FIXED (2026-08-26).** Read this before
   touching fleet links. `adopt_direct` inferred the Fleet intent from the local
   `DirectPending`, which only the DIALER has, so every ACCEPTED fleet link was
   born `trusted: true` + OwnerDevice. Default-on, not limited to the opt-in send
   path. Caught live: a device whose `transfer` grant had been REVOKED still
   delivered a file (sha256 verified), because the acceptor granted it
   owner-equivalence at link birth, before capabilities were consulted. Fixed by
   failing safe with no pending: a peer we hold a PAIR secret for is that device;
   anything else, while a fleet secret exists, stays untrusted until
   `fleet-hello` names it. Re-verified: mesh still forms on all three nodes, and
   the revoked peer's file no longer lands. Full write-up in
   design-fleet-automesh.md.

1b. **`send --to` to a fleet sibling: MISDELIVERY FIXED. Opt-in, still OFF.**
   Root cause: `fleet_proven` was a single bool meaning "some peer proved", read
   everywhere as "THIS peer proved". With several siblings on the fleet channel,
   one peer verifying opened the offer guard for all of them. Measured: verified
   pid=8gNe as 'laptop', offered to pid=yNDj (the owner), which accepted.
   Fixed by making it a per-peer set. Re-verified in the same scenario: verified
   pid and offered pid now match, the offer is declined, and NO node receives
   anything.
   SECOND TOPOLOGY CONFIRMS (2026-08-26), with per-node inboxes:
     joined -> joined (C -> laptop): verify pid == OFFERING pid, declined,
                                     NO node received anything.
     joined -> owner  (C -> owner):  delivered to the owner's inbox only, which
                                     is correct (they are paired; the normal
                                     path, untouched by the fleet work).
   So the per-peer fix holds across sender/target roles and the ordinary paired
   send is unaffected. The warning was corrected from "KNOWN to deliver to the
   WRONG device" (no longer true) to "experimental, one topology".

   STAYS OPT-IN, deliberately. The path is now CORRECT but not USEFUL: a transfer
   to a sibling still cannot succeed, because a joined device has no owner-signed
   capability state to authorize it (item 2). Flipping the flag on today would
   replace a clear "not yet a send target" message with a decline, which is worse
   for the user. Enable it when item 2 lands, not before.

1b-old. **(withdrawn framing kept for the reasoning) BLOCKED on the trust floor**
   CORRECTION to an earlier note in this file: I recorded this as "WORKS, end to
   end, sha256 verified". That result was real but it was produced by the
   escalation in 1a. With the escalation fixed, it does not deliver, and the
   earlier success was the bug rather than the feature.

   What IS built and verified: certificate-verified dialing of a sibling,
   wrong-sibling rejection (measured: asked for `laptop`, `owner` answered, and
   it refused rather than misdeliver), and offers gated on identity.

   THE BLOCKER (corrected). An earlier version of this entry said "blocked on the
   trust floor". That was WRONG: `cap_trust_floor` already passes when
   `binding == Proven`, which a verified `fleet-hello` sets. Do not redesign the
   trust model on the strength of that sentence.

   What is true:
   - DEFAULT (shadow) mode falls back to `legacy_ok`, which in daemon mode is
     `link_trusted`. A fleet link is untrusted by design, so it declines. That is
     section 4b working, not a defect.
   - Under `FILAMENT_CAP_AUTHORITATIVE=1` the capability layer ALREADY has a
     fleet transfer path (`cap_fleet_inputs` + `scoped_in_bounds`): a same-owner
     Proven device may land a file inside the receiver's own drop dir. The
     mechanism exists and was built for this case.

   ROOT CAUSE FOUND AND FIXED, then a SECOND one found (2026-08-26).

   FIXED: the offer was never emitted. `send_cmd` defers offers until identity is
   proven, but the offer lives in the `ChannelReady` handler, which had already
   run and deferred. Setting `fleet_proven` without RE-ENTERING that handler
   meant the offer never happened at all: sender sat until timeout, receiver's
   gate never saw a request. The PAKE path documents the same remedy for itself
   ("sets `pake_done` and re-emits ChannelReady to fall through here and offer");
   the fleet path now re-emits too. Verified: the offer reaches the gate.

   REMAINING, and it is a policy-store gap, not plumbing. With the offer
   arriving, the receiver's gate reports:

       transfer-gate: allowed=false binding=Proven own_user=false has_grant=false
                      in_bounds=true revoked=false authoritative=true

   `own_user=false` is the blocker. `cap_fleet_inputs` derives it from a
   `cap_header` in the capability store, and a JOINED device does not have one:
   measured, the owner device has `caps.json` with a cap_header, the joined
   device has no `caps.json` at all. So the fleet-scope transfer path (the one
   built for exactly this case) can never engage on a joined device, and the
   `has_grant` path cannot either, because grants are evaluated against that same
   store.

   ITEM 2 PROGRESS, measured (2026-08-26/27):
     - `cap_header` delivered at enrollment: WORKS. Gate now reports
       `own_user=true` where it was `false` before. That is the header doing
       exactly its job.
     - `cap_ratchet` initialised on the joined device: WORKS. An owner has
       `{cap_header, cap_ratchet}`; a joined device had only the header, which is
       half of what `ensure_self_genesis_header` writes. The ratchet needs no
       signature (local anti-rollback keyed on owner_pub, which the header
       carries), so the joiner creates its own. Verified: B's store now has both.
     - End-to-end transfer to a sibling WORKS, with an explicit grant:

           filament grant <sibling> transfer     (on the receiver)
           gate: allowed=true legacy_ok=false trusted=false binding=Proven
                 own_user=true has_grant=false in_bounds=true
           inboxes: owner[]  laptop[gr.txt]      <- the NAMED device

       `legacy_ok=false` and `trusted=false` matter: the link is NOT trusted, so
       this was authorized purely by the capability layer, and the file reached
       the device the user named rather than the owner.

     WHY THE EARLIER DENIALS WERE CORRECT, not a bug. Answered by READING
     `cap_gate_effective` rather than more instrumenting: the delegated-principal
     ceiling (check 2 of 2) is unconditional in both modes and purely
     restrictive. A fleet link is admitted with `device_caps(name)` as its
     ceiling, which is `Some([])` for a sibling with no grant, so it denies
     before the fleet-scope branch is ever consulted. Every earlier measurement
     was taken WITHOUT a grant. That is section 4b working exactly as designed:
     reachability by default, capability only on an explicit grant.

     So item 2's first two increments (header + ratchet delivery) are sufficient
     for granted transfer. What remains of item 2 is the ungranted fleet-scope
     case and grant DISTRIBUTION, not this.

   ORIGINAL NOTE. FIRST INCREMENT OF ITEM 2 IS LANDED (2026-08-26): the owner now hands its
   signed `cap_header` to a joining device in the enrollment ack, and both join
   paths store it. VERIFIED: after a fresh three-node enrollment all three
   devices have a `cap_header`, where previously only the owner did. The header
   carries no secret (signed, self-certifying, names `owner_pub` + nonce), and
   authoritative mode is off by default, so default behaviour is unchanged.

   CHARACTERISED, and a false alarm RETRACTED. Correcting two earlier notes in
   this file, both of which were wrong.

   Wrong note 1: "timing-dependent / flaky". Repeated runs against one stable
   fleet in default (shadow) mode are 3/3 identical:
       verifying fleet identity -> declined
   Consistent, not flaky.

   Wrong note 2, and this is the important one. Three authoritative-mode runs
   showed run 1 DELIVERING a file with zero `transfer-gate:` lines, which I began
   recording as a suspected authorization hole. IT IS NOT. The test harness was
   at fault: all three nodes had `dir` unset, so A, B and C shared ONE inbox
   (`/root/Filament`). "A file appeared in the inbox" therefore could not
   identify WHICH node accepted it, and C is legitimately paired with A, so a
   file A accepted over its normal trusted link looked exactly like B accepting
   one without a gate check.

   Re-run with a distinct inbox per node:
       verifying fleet identity -> declined,  and NO node received anything.

   So the behaviour is correct and there is no hole. Recorded at length because
   the near-miss is the lesson: the measuring instrument could not tell the
   nodes apart, and it produced a false security finding that was one step from
   being written down as real. Any future fleet test MUST set a distinct `dir`
   per node before its results mean anything.

   ORIGINAL CLOSING NOTE: this IS item 2, not a separate bug. A joined device
   cannot simply create the missing header. `ensure_self_genesis_header` SIGNS it
   with the owner's `UserKey` (`sign_cap_header`), which a joined device does not
   hold and cannot synthesize. The header is an owner-signed artifact, so a fleet
   device can only RECEIVE one, the way it receives `fleet_rv`, and it would also
   need owner-signed grant entries to evaluate `has_grant` at all.

   That is precisely the deferred design, and the codebase says so itself: the
   `cap_authoritative` doc comment calls the env var "the opt-in switch, pending
   same-owner fleet-trust that makes the default-on flip safe". Distributing
   owner-signed capability state to fleet devices IS same-owner fleet-trust.

   So the two open items collapse into one. Do item 2 (owner-signed fleet
   ceiling / policy distribution) and `send --to` for siblings follows from it;
   there is nothing separate left to fix in the transfer path. Everything
   upstream of the capability store now works and is measured: dial, certificate
   verification, wrong-sibling rejection, offer emission, and delivery of the
   offer to the receiver's gate.

   (superseded note follows)
   ANSWERED (2026-08-26). The receiver's transfer gate is now instrumented (a
   `transfer-gate:` debug line carrying allowed / legacy_ok / trusted / binding /
   own_user / has_grant / in_bounds / revoked / authoritative / reason, visible
   at `-vv`). Run with the receiver in authoritative mode and a `transfer` grant
   in place: the line NEVER FIRES. The offer does not reach the capability layer
   at all, so nothing about caps, floors or grants is responsible.

   The blocker is UPSTREAM of the gate: either the sender never emits the
   file-offer, or the link does not carry it. Note the sender also printed
   nothing under `-vv` before its timeout, which points at the sender side.
   Start there, not in capability.rs: find where `send_cmd` stops after
   "verifying fleet identity" succeeds, i.e. whether it ever reaches its offer
   emission for a fleet target. The gate instrumentation is kept (debug-level,
   one line per offer) because it is what turned a plausible wrong answer into
   a measured one.

   The opt-in stays OFF; `send --to <sibling>` keeps the accurate
   "not yet a send target" message.

1c. **(historical: the earlier assessment that this needed its own session)**
   Set `FILAMENT_FLEET_SEND=1` to exercise it. Off by default because a
   half-finished feature should not be the default path; without it the clear
   "not yet a send target" message is what users get.

   WHAT WORKS AND IS VERIFIED LIVE:
   - `send_cmd` resolves a fleet sibling as a target and dials it on the fleet
     secret / fleet channel.
   - Offers are GATED on the peer's certificate, so nothing is offered to an
     unproven peer.
   - Wrong-sibling detection works and was measured: asked for `laptop`, the peer
     that answered first was `owner`, and it refused with "proved a different
     identity. Nothing was sent." Without that check the file would have gone to
     the wrong device. This is the single most valuable part of the work.
   - After the skip-and-keep-looking fix, the sender then reaches the RIGHT peer.

   WHAT IS NOT FINISHED: the binding handshake on a transport with no RFC-5705
   exporter. The peer's `fleet-hello` can arrive before the sender has set up its
   challenge nonce, so `fleet_bind_ours` is still None and verification fails
   closed with "cannot verify its certificate". Fails SAFE (never misdelivers),
   but does not complete. The daemon solves the same problem in its ChannelReady
   path; the sender needs the equivalent ordering, which most likely means
   establishing the nonce before the link can deliver control messages rather
   than on ChannelReady.

   Also landed and independently useful: `bring_up_to_known` (l2.rs) now dials a
   fleet sibling and calls `verify_fleet_identity` before returning the link, so
   the COLD paths (ssh / netcat / forward / pty) get certificate-checked fleet
   dialing.

1c. **(superseded assessment, kept for the reasoning)**
   This is the last gap for fleet peers (every other verb works: see the
   verb-by-verb table in design-fleet-automesh.md). It needs the daemon to run a
   transfer over a link the one-shot cannot dial, which means a new warm-send
   request kind AND a transfer core that can run over an EXISTING transport.

   The cost is the reason it is not done. `send_cmd` is `cli/src/main.rs`
   13077-14356, about 1280 lines, and it interleaves connection establishment,
   the PAKE ceremony, its own event loop, offers, chunking, resume and progress
   UI. Nothing separates "set up a link" from "move the bytes", so a warm-send
   means extracting the transfer core out of the most safety-critical path in the
   product, one with a documented corruption history and its own resilience
   gates. Duplicating it into the daemon instead would be worse.

   **The cheap-looking alternative is worse, do not take it.** The obvious
   shortcut is to skip the daemon entirely and let the ONE-SHOT dial the sibling
   itself: `fleet.rv` is on disk, so `bring_up_to_known` (l2.rs:1048) could fall
   back to the fleet secret and the fleet channel when there is no pair secret,
   and every verb would work with no `send_cmd` change at all.

   It breaks on identity. A pair channel has exactly ONE peer on it, which is why
   that function can dial "the known device" it finds there. The FLEET channel has
   every sibling on it, so the one-shot would have to pick by the peer's
   ADVERTISED presence name, which is self-asserted and unverified at that point.
   Any fleet member could answer to "laptop" and receive the file. The daemon does
   not have this problem because it runs `fleet-hello` and checks the certificate
   before admitting; a one-shot taking this shortcut would be trusting a name.
   (This is the same trap as 4c in design-fleet-automesh.md: a proven key is not a
   proven name. It has now bitten in three separate places.)

   So the shortcut also needs certificate verification on the sending side, which
   is most of the work it was supposed to avoid. Either route needs identity
   verification before bytes move; the daemon route at least reuses the one that
   already exists and is tested.

   That refactor deserves its own session, run against the transfer gates and the
   test-record pipeline, not a tail-end change. Meanwhile the gap is signposted
   rather than silent: `send --to <sibling>` names the limitation and points at
   the two things that DO work (the peer's `.mesh` address, or pairing directly).

1d. **Ungranted fleet-scope transfer needs a NEW PRINCIPAL KIND. Analysed, not
   started.** The capability layer already implements the policy: a same-owner
   Proven device may land a file inside the receiver's own drop dir without a
   grant (`cap_fleet_inputs` + `scoped_in_bounds`). It cannot fire for a fleet
   link, and the reason is structural rather than a tweak:

   - `cap_gate_effective` applies the delegated ceiling only `if let Some(caps) =
     auth_key_caps`. A `Delegated` principal always has a ceiling; an empty one
     denies everything before fleet-scope is consulted.
   - So reaching fleet-scope requires `auth_key_caps == None`, i.e. NOT
     `Delegated`. The only other kind is `OwnerDevice`, which is owner-equivalent
     and is exactly the escalation removed earlier in this session.

   There is no kind that means "identity proven, no ceiling, NOT owner". That is
   what ungranted fleet-scope needs, and inventing it is a security-model change,
   not a one-line default flip. Note this also means the conservative empty
   ceiling chosen in 4b is not merely cautious: given the current kinds, it is
   the only safe option.

   Granted transfer already works today and is verified, so this affects the
   no-grant convenience case only.

2. **Owner-signed fleet ceiling.** Capabilities do not flow across a fleet today;
   every grant stays explicit. An owner-signed default ceiling delivered at
   enrollment is the shape, deferred deliberately.
3. **L3-over-relay performance.** The DataChannel is reliable and ordered, so the
   tunnel head-of-line blocks and shares the channel with file transfer. Fine as
   a fallback, worth a second unreliable channel if it ever carries real load.
