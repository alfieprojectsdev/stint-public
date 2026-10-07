"""M0 parity test: stintcore.engine must be byte-identical to the bash `stint.sh`.

Strategy: a bash oracle (tests/oracle.sh) runs the ORIGINAL formulas verbatim
(date/bc/printf/cut). For a wide grid of inputs we assert the Python port emits
exactly the same duration, billable amount, and CSV row. If bash and Python ever
disagree on the last rounded digit, this fails — which is the whole point.
"""

from __future__ import annotations

import subprocess
from datetime import datetime, timedelta
from decimal import Decimal
from pathlib import Path

import pytest

from stintcore import config, engine

ORACLE = Path(__file__).parent / "oracle.sh"

# A representative spread of durations (seconds), chosen to exercise:
#   * sub-0.25h entries (round up to a full quarter)
#   * the 15-minute / quarter-hour boundaries (900s, 1800s, ...)
#   * long multi-hour blocks
#   * values whose 4dp truncation lands on rounding boundaries
_DURATION_SECONDS = [
    0, 1, 5, 30, 59, 60, 89, 90, 120, 161, 162, 163, 300, 449, 450, 451,
    599, 600, 899, 900, 901, 1234, 1799, 1800, 2400, 3599, 3600, 3601,
    5000, 7200, 9000, 12345, 18000, 25200, 36000, 40000, 43200,
]

# Second-counts where legacy bash `printf` rounds a half-cent-hour boundary
# incorrectly (double-rounding artifact — see engine._printf_2f). The port rounds
# correctly and therefore INTENTIONALLY differs here; these are asserted
# separately in test_legacy_bash_rounding_divergences, not treated as parity
# failures. 162s = 0.045h: bash -> 0.05, correct -> 0.04.
_LEGACY_ROUNDING_DIVERGENCES = {162}

_PARITY_SECONDS = [s for s in _DURATION_SECONDS if s not in _LEGACY_ROUNDING_DIVERGENCES]

_BASE = datetime(2026, 7, 8, 9, 0, 0)


def _oracle(*args: str) -> str:
    return subprocess.run(
        ["bash", str(ORACLE), *args],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.rstrip("\n")


def _ts(dt: datetime) -> str:
    return dt.strftime("%Y-%m-%d %H:%M:%S")


@pytest.mark.parametrize("secs", _PARITY_SECONDS)
def test_duration_matches_bash(secs: int) -> None:
    start = _ts(_BASE)
    end = _ts(_BASE + timedelta(seconds=secs))
    assert engine.duration_hrs(start, end) == _oracle("hrs", start, end)


@pytest.mark.parametrize("secs", _PARITY_SECONDS)
def test_stop_row_matches_bash(secs: int) -> None:
    start_dt = _BASE
    end_dt = _BASE + timedelta(seconds=secs)
    start, end = _ts(start_dt), _ts(end_dt)
    desc = "PR #129 review — diff + cross-repo audit (example-org/example-repo)"

    # boots=[] => no reboot cap, matching the oracle's no-cap assumption.
    result = engine.stop(
        entry_date=start_dt.strftime("%Y-%m-%d"),
        start_time=start,
        category="pr",
        description=desc,
        end_time=end,
        boots=[],
    )
    expected = _oracle(
        "row", start_dt.strftime("%Y-%m-%d"), start, end, "pr", desc
    )
    assert result.csv_row == expected


@pytest.mark.parametrize(
    "desc",
    [
        "plain description",
        "has, commas, in it",
        "em — dash narrative — with, comma",
        "T10 scraper impl (example-org/example-repo)",
        "unicode: résumé café Nguyễn",
    ],
)
def test_description_passthrough_matches_bash(desc: str) -> None:
    start = _ts(_BASE)
    end = _ts(_BASE + timedelta(minutes=40))
    result = engine.stop(
        entry_date="2026-07-08",
        start_time=start,
        category="dev",
        description=desc,
        end_time=end,
        boots=[],
    )
    expected = _oracle("row", "2026-07-08", start, end, "dev", desc)
    assert result.csv_row == expected


@pytest.mark.parametrize(
    "hrs",
    ["0.25", "0.50", "0.67", "1.07", "6.18", "42.10", "0.10", "100.00"],
)
def test_billable_matches_bash(hrs: str) -> None:
    result = engine.StopResult(
        entry_date="2026-07-08",
        start_time="2026-07-08 09:00:00",
        end_time="2026-07-08 09:40:00",
        hrs=hrs,
        category="dev",
        description="x",
        reboot_capped=False,
    )
    assert result.billable == _oracle("bill", hrs)


def test_quarter_roundup_boundary() -> None:
    # 14m -> 0.25 (short session bills a full quarter); exactly 15m -> 0.25 (on
    # the mark, unchanged); 15m01s -> 0.50 (rounds up to the next quarter).
    start = _ts(_BASE)
    under = _ts(_BASE + timedelta(minutes=14))
    at = _ts(_BASE + timedelta(minutes=15))
    over = _ts(_BASE + timedelta(minutes=15, seconds=1))

    r_under = engine.stop("2026-07-08", start, "pr", "x", end_time=under, boots=[])
    r_at = engine.stop("2026-07-08", start, "pr", "x", end_time=at, boots=[])
    r_over = engine.stop("2026-07-08", start, "pr", "x", end_time=over, boots=[])

    assert r_under.hrs == "0.25"
    assert r_at.hrs == "0.25"
    assert r_over.hrs == "0.50"
    # And all agree with bash.
    assert r_under.csv_row == _oracle("row", "2026-07-08", start, under, "pr", "x")
    assert r_at.csv_row == _oracle("row", "2026-07-08", start, at, "pr", "x")
    assert r_over.csv_row == _oracle("row", "2026-07-08", start, over, "pr", "x")


def test_reboot_cap_uses_first_boot_after_start() -> None:
    start = "2026-07-08 09:00:00"
    real_end = "2026-07-08 17:00:00"  # 8h if uncapped
    boot = int(datetime(2026, 7, 8, 9, 30, 0).timestamp())  # machine rebooted 09:30

    result = engine.stop(
        "2026-07-08", start, "dev", "spanned a reboot",
        end_time=real_end, boots=[boot],
    )
    assert result.reboot_capped is True
    assert result.end_time == "2026-07-08 09:30:00"
    assert result.hrs == "0.50"  # capped to 30m, not billed for 8h downtime

    # Row matches the oracle when the oracle is fed the CAPPED end.
    expected = _oracle("row", "2026-07-08", start, "2026-07-08 09:30:00", "dev", "spanned a reboot")
    assert result.csv_row == expected


def test_rate_single_source() -> None:
    # The whole reason for M0: rate is defined once. Guard against silent drift.
    assert config.RATE == Decimal("16.00")


def test_first_boot_after_selects_earliest() -> None:
    assert engine.first_boot_after(100, [50, 150, 200, 175]) == 150
    assert engine.first_boot_after(100, [50, 90]) is None
    assert engine.first_boot_after(100, []) is None


@pytest.mark.parametrize("secs", sorted(_LEGACY_ROUNDING_DIVERGENCES))
def test_legacy_bash_rounding_divergences(secs: int) -> None:
    """The port INTENTIONALLY differs from legacy bash at these boundaries.

    This is a documented decision, not a regression: bash's builtin printf
    double-rounds half-cent-hour values (engine._printf_2f). We assert the exact
    divergence so it stays visible and reviewable — if bash and the port ever
    agree here (e.g. a future bash), this test flips and prompts a re-decision."""
    start = _ts(_BASE)
    end = _ts(_BASE + timedelta(seconds=secs))
    port = engine.duration_hrs(start, end)
    bash = _oracle("hrs", start, end)
    assert port != bash, (
        f"{secs}s: port and bash now AGREE ({port}); "
        "remove from _LEGACY_ROUNDING_DIVERGENCES and re-check the invoice math."
    )
    # The divergence is exactly one cent-hour, i.e. <= $0.16 per entry at $16/hr.
    delta = abs(Decimal(port) - Decimal(bash))
    assert delta == Decimal("0.01")


def test_162s_divergence_is_the_045_boundary() -> None:
    # Pin the canonical example so the finding is self-documenting.
    start = _ts(_BASE)
    end = _ts(_BASE + timedelta(seconds=162))  # 162/3600 = 0.045h exactly
    assert engine.duration_hrs(start, end) == "0.04"  # correct: nearest double < 0.045
    assert _oracle("hrs", start, end) == "0.05"  # legacy bash double-rounds up
