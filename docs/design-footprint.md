# Startup cost and footprint

## Why this document exists

"It takes a while to start" was reported and could NOT be reproduced on the
development box: every local command measured 40-57ms cold. Guessing from that
position wastes the reporter's time, so the first deliverable was an instrument
rather than a fix: `experiments/startup-bench.sh` and
`experiments/footprint.sh`, both runnable on the machine that actually feels
slow.

## What startup was actually spending

`strace -c` on `filament --version`, which does nothing but print a string:

    4 clone3      227 syscalls      7.0ms of syscall time

Four threads, on a four-core box, to print a version string. `#[tokio::main]`
builds a MULTI-THREADED runtime unconditionally, spawning one worker per CPU
before `main` runs. The cost therefore scales with core count: the bigger the
machine, the slower the trivial commands feel, which is the opposite of what
anyone expects and is a good candidate for the original report.

The fix is to pick the runtime AFTER looking at the command
(`is_light_command`). Trivial, local-only verbs get a current-thread runtime;
everything else, including anything unrecognised or added later, keeps exactly
today's multi-threaded behaviour. The polarity is deliberate: a wrong answer is
a throughput question, never a correctness one, because a current-thread runtime
still runs every future and still has a blocking pool. The single API that would
panic there is `block_in_place`, and the tree contains none.

    after: 0 clone3      116 syscalls      2.0ms of syscall time

## Binary size

`size -A` said where 28.7MB lived: `.text` 21.2MB, then 3.6MB of UNWINDING
tables (`.eh_frame` + `.gcc_except_table`).

`lto = "fat"` with `codegen-units = 1` took the binary from **28.7MB to 19.5MB**
and, because better inlining also means less to relocate and page in, took
startup down with it. Release build time roughly doubled (5m20s to 10m40s),
which is paid by CI and by nobody else.

`panic = "abort"` would have removed most of those 3.6MB and was REJECTED, not
overlooked. `cli/src/tun/netstack.rs` wraps `iface.poll` in `catch_unwind`
specifically so a packet-level panic on hostile input stays contained. With
`panic = "abort"` a malformed packet from a peer would abort the process
instead: a remote denial of service traded for 12% of a binary.

## Measured, before and after

| metric | before | after |
|---|---|---|
| binary | 28.7 MB | **19.5 MB** |
| `--version`, warm | 10.5 ms | **8.4 ms** |
| minus the 4.2ms process floor | 6.3 ms | **4.2 ms** |
| threads spawned for `--version` | 4 | **0** |
| one-shot peak RSS | 10.4 MB | **9.3 MB** |
| daemon idle RSS | 25.8 MB | **22.0 MB** |
| daemon idle CPU | 0 ticks/10s | 0 ticks/10s |

## The metrics this kind of tool is judged on

Throughput is the one benchmarks report and the one users notice least. For a
tool with a daemon half, in rough order of how much they matter:

1. **Idle RSS.** A daemon holds it 24/7, and it decides whether the tool belongs
   on a small box at all.
2. **Idle CPU / wakeups.** The battery metric. A daemon that wakes constantly is
   worse than one using more RAM, and it never appears in a throughput
   benchmark. Filament is at 0 ticks per 10 idle seconds, meaning it genuinely
   waits rather than polls. This is already good and worth not regressing.
3. **Binary size.** What you ship, and the hard limit on flash-constrained
   targets.
4. **Startup latency.** Paid on every single CLI invocation.
5. **Per-peer scaling** of threads, fds and memory. Fine at one peer, decisive
   at fifty.
6. **Peak RSS under load**, which is where buffer sizing shows up.
7. **CPU per byte** moved, which is what actually limits throughput on a small
   ARM core.

## Cutting it further: what a second look found

An earlier version of this document said 19.5MB was a dependency problem that "no
profile flag closes". That was a conclusion drawn after trying two flags, and it
was wrong. `cargo bloat --crates` and `cargo tree` found three more cuts:

**A crypto library we never call: 2.6MB.** `cargo bloat` put `aws_lc_sys` at
1.3MB of `.text`, third largest crate in the binary. `main.rs` already explains
why it is there: two providers land in the tree and rustls refuses to guess, so
the code installs `ring` explicitly. But `rustls` was declared as
`features = ["ring"]` WITHOUT `default-features = false`, so rustls compiled its
default provider too, and `reqwest`'s default TLS pulled a third path into it.
Setting `default-features = false` on rustls and switching reqwest to
`rustls-no-provider` (we install the provider ourselves) removed `aws-lc-rs` and
`aws-lc-sys` from the graph entirely: **19.5MB to 16.9MB**. Verified by a real
transfer over the real signaling server, because a crypto change that merely
compiles has proved nothing.

**Cold crates optimised for size: 1.3MB.** `opt-level = "z"` costs throughput, so
it is applied per-package to crates that are not on the bulk-data path: argument
parsing, the HTTPS control plane, and WebRTC session setup. `ring`, `quinn`,
`tokio`, `smoltcp` and filament's own crates stay at full optimisation.
**16.9MB to 15.6MB**, and throughput was re-measured on the two-machine rig
rather than assumed: 4 runs each of a 30MB transfer, median 18.1 MB/s before and
18.6 MB/s after, with run-to-run variance far exceeding the difference.

**Duplicate dependency versions.** 30 crate names appear at more than one
version. Most are small, but `reqwest` is there twice (0.12 via `rust_socketio`,
0.13 ours). `hyper`, `h2`, `rustls` and `ring` unified to single versions once
the TLS features were fixed, which removed most of the real cost; the remaining
reqwest duplication is roughly 200KB and would need `rust_socketio` to move.

## Totals

| metric | start | now |
|---|---|---|
| binary | 28.7 MB | **15.6 MB** (-46%) |
| `--version` warm | 10.5 ms | **8.5 ms** |
| threads for `--version` | 4 | **0** |
| one-shot peak RSS | 10.4 MB | **8.8 MB** |
| daemon idle RSS | 25.8 MB | **21.5 MB** |
| daemon idle CPU | 0 ticks/10s | 0 ticks/10s |


## Calibration: is 15.6MB actually big?

The earlier comparison in this document was against tmux (1.1MB) and it was
MISLEADING, in a way worth spelling out because it is a common mistake. tmux is
a dynamically-linked C binary: the 1.1MB is the part that is not libc, ncurses,
libssl and so on. Its real closure, binary plus the shared libraries it loads,
is 8.1MB. Measured the same way on the same machine:

| tool | binary | + its shared libs |
|---|---|---|
| curl | 0.3 MB | **19.4 MB** |
| tcpdump | 1.2 MB | 13.3 MB |
| ssh | 0.8 MB | 10.4 MB |
| nginx | 1.3 MB | 10.1 MB |
| openssl | 1.0 MB | 8.9 MB |
| tmux | 1.1 MB | 8.1 MB |

So curl, the canonical "small" tool, actually pulls in MORE than filament does.

Against tools in filament's own class, which carry their own TLS, crypto and
protocol stacks rather than borrowing the system's:

| tool | size | what it does |
|---|---|---|
| node | 118.9 MB | JS runtime |
| dockerd | 99.0 MB | container daemon |
| containerd | 45.9 MB | container runtime |
| docker (CLI) | 43.5 MB | client only |
| **tailscaled** | **40.9 MB** | mesh VPN daemon |
| cc1 (GCC's actual compiler) | 32.6 MB | C compiler backend |
| **tailscale (CLI)** | **31.6 MB** | mesh VPN client |
| rustc / cargo | 19.9 MB | compiler / build tool |
| **filament** | **15.6 MB** | mesh + transfer + mount + shell |
| croc | 14.9 MB | file transfer only |

filament is **half the size of the Tailscale CLI and 2.6x smaller than
tailscaled**, while doing more than either: the mesh, plus file transfer, plus a
FUSE mount, plus a web shell. It is within a megabyte of croc, which only
transfers files. `gcc` is a 1MB driver that execs `cc1`, and `cc1` is 32.6MB, so
"as big as a C compiler" would in fact be twice filament's size.

The conclusion is not that size stopped mattering. It is that the target should
be croc's ~15MB rather than tmux's apparent 1.1MB, and filament is already there.

## The last dependency that keeps it from being self-contained

filament links the system `libssl`/`libcrypto` (its closure is 24.7MB), which
`croc` and `tailscale` do not. The cause is recorded in `cli/Cargo.toml`:
`rust_socketio` hard-depends on `native-tls`, so a rustls-only tree needs that
crate forked or replaced (ledger C16). The same crate is also the one pinning the
duplicate `reqwest` 0.12. Removing that single dependency would drop the system
TLS libraries AND the duplicate HTTP stack, and make the binary genuinely
portable in the way the Go competitors are. That is the highest-value remaining
item and it is a dependency decision, not a build flag.

## Where the remaining weight is, honestly

15.6MB against tmux's 1.1MB is still an order of magnitude, and no
profile flag closes that. The weight is dependencies: 509 crates, including
`webrtc`, `quinn`, `smoltcp`, `reqwest`, `portable-pty`, `zip`, `tar`,
`qrcode` and `clap_mangen`, plus `tokio` with `features = ["full"]`.

Getting genuinely small is therefore a FEATURE-GATING problem, not an
optimisation problem: a `--no-default-features` build that drops the web shell
(portable-pty), FUSE mount (fuser), archive support (zip/tar), QR rendering and
man-page generation, and ideally offers a relay-free build without `webrtc`.
That is a real piece of work with a real payoff and it has not been done. It
should be measured with `experiments/footprint.sh` before and after, not
estimated.
