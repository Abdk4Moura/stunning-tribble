#!/usr/bin/env python3
"""Model the stall correction ladder, including what each rung destroys.

The ladder retries a stuck transfer five times, with a 15-second watchdog per
rung. This model does not assume that "stuck while connecting" has one cause:
transience and failure type are explicit environment parameters. A retry can
only benefit from a transient condition if it preserves the state that the
next attempt needs.

Run this file directly. Gate 0 first reproduces the observed #31, #50, and #38
outcomes before the model prints any recommendation.
"""
from dataclasses import dataclass
from itertools import product


MAX_ATTEMPTS = 5
WATCHDOG_SECS = 15
TRANSIENT, PERSISTENT = "transient", "persistent"

FAILURE_STATE = {
    "data-freeze": "transport",
    "ice-connect": "ice",
    "roster-re-adoption": "roster",
}


@dataclass(frozen=True)
class Environment:
    transience: str
    failure_type: str
    discarded_state: frozenset[str]


@dataclass(frozen=True)
class Result:
    recovered: bool
    attempts: int
    elapsed_secs: int
    terminal: str


def run_ladder(environment: Environment, attempts=MAX_ATTEMPTS) -> Result:
    """Run the ladder; return whether the ladder itself recovered the transfer."""
    required = FAILURE_STATE[environment.failure_type]
    for rung in range(1, attempts + 1):
        # A persistent condition never clears. A transient condition clears after
        # the first failed cycle, but only retained state can make that useful.
        condition_cleared = environment.transience == TRANSIENT and rung > 1
        state_available = required not in environment.discarded_state
        if condition_cleared and state_available:
            return Result(True, rung, rung * WATCHDOG_SECS, "recovered")
    return Result(False, attempts, attempts * WATCHDOG_SECS, "ladder-exhausted")


CALIBRATIONS = {
    "#31 data freeze": Environment(PERSISTENT, "data-freeze", frozenset({"transport"})),
    "#50 NAT ICE": Environment(PERSISTENT, "ice-connect", frozenset({"ice", "conntrack"})),
    "#38 roster re-adoption": Environment(PERSISTENT, "roster-re-adoption", frozenset({"roster"})),
}


def gate_0():
    """Reproduce the observed terminal behavior before making recommendations."""
    expected = {name: (False, MAX_ATTEMPTS, MAX_ATTEMPTS * WATCHDOG_SECS) for name in CALIBRATIONS}
    failures = []
    for name, environment in CALIBRATIONS.items():
        result = run_ladder(environment)
        if (result.recovered, result.attempts, result.elapsed_secs) != expected[name]:
            failures.append(f"{name}: expected {expected[name]}, got {result}")
    if failures:
        raise SystemExit("GATE 0 FAILED: model does not reproduce observations\n" + "\n".join(failures))
    print("GATE 0: PASS (#31, #50, #38 all exhaust 5 rungs without ladder recovery)")


def explore():
    """Show whether any modeled environment lets a later rung recover."""
    outcomes = []
    for transience, failure_type, discarded in product(
        (PERSISTENT, TRANSIENT),
        sorted(FAILURE_STATE),
        (frozenset(), frozenset(FAILURE_STATE.values())),
    ):
        result = run_ladder(Environment(transience, failure_type, discarded))
        outcomes.append((transience, failure_type, bool(discarded), result))
    return outcomes


def main():
    gate_0()
    for name, environment in CALIBRATIONS.items():
        result = run_ladder(environment)
        external = "; #38 later recovery is external roster adoption" if name == "#38 roster re-adoption" else ""
        print(f"{name}: {result.attempts} attempts, {result.elapsed_secs}s, {result.terminal}{external}")

    later_rung_successes = [
        row for row in explore() if row[3].recovered and row[3].attempts > 1
    ]
    print(f"TRANSIENT + RETAINED STATE: later-rung recoveries={len(later_rung_successes)}")
    print("CALIBRATED REGIME: persistent failures with discarded recovery state")
    print("RECOMMENDATION: do not raise MAX_ATTEMPTS or WATCHDOG_SECS; the observed ladder is delay, not recovery.")


if __name__ == "__main__":
    main()
