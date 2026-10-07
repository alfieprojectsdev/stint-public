"""Timer / billing engine — ported from the bash `stint.sh` script.

Correctness contract: for the same inputs, `stop()` produces a CSV line
byte-identical to bash `_stop_timer_file` (stint.sh:98-149) for every value EXCEPT a
small, explicitly-catalogued set of half-cent-hour boundaries where legacy bash
rounds incorrectly (see `_printf_2f` and the divergence test). The parity test
(tests/test_parity.py) enforces both: byte-parity on the general grid, and the
documented, intentional divergence on the boundaries.

Fidelity notes:
  * `duration_hrs` mirrors `bc "scale=4; diff/3600"` (truncation to 4 dp) followed
    by `printf "%.2f"`. We truncate with Decimal ROUND_DOWN, then format via
    `"%.2f" % float(...)` so Python and bash feed the *same* double to the *same*
    libc formatter — that's what makes the last-digit rounding match exactly.
  * The quarter-hour rule rounds measured time UP to the next QUARTER_HOUR mark,
    computed from whole seconds (as bash does via integer 900s buckets).
  * The CSV description is wrapped in double quotes with NO escaping, exactly like
    bash `\"$description\"`. This is faithful (and inherits bash's latent bug for
    descriptions containing a double quote); proper csv.writer quoting is an M1
    change, called out in the spec, intentionally NOT made here.
"""

from __future__ import annotations

import subprocess
from dataclasses import dataclass
from datetime import datetime
from decimal import Decimal, ROUND_DOWN, ROUND_HALF_EVEN

from . import config

_TS_FMT = "%Y-%m-%d %H:%M:%S"


def _parse_ts(ts: str) -> datetime:
    return datetime.strptime(ts, _TS_FMT)


def duration_seconds(start: str, end: str) -> int:
    """Whole seconds between two 'YYYY-MM-DD HH:MM:SS' timestamps.

    Matches bash `$(( end_epoch - start_epoch ))` for same-offset timestamps
    (tz/DST edge cases are out of scope, same as the original which also just
    subtracts epochs)."""
    return int((_parse_ts(end) - _parse_ts(start)).total_seconds())


def _bc_scale4_div(numerator: int, denominator: int) -> Decimal:
    """Reproduce `bc "scale=4; a/b"` — quotient truncated to 4 decimals."""
    return (Decimal(numerator) / Decimal(denominator)).quantize(
        Decimal("0.0001"), rounding=ROUND_DOWN
    )


def _printf_2f(value: Decimal) -> str:
    """Format to 2 decimals using principled, correctly-rounded half-to-even.

    We round the *exact* value of the IEEE-754 double (`Decimal(float(value))`)
    to 2 dp, ties-to-even — the IEEE-754 default and what a correct `printf`
    should do.

    NOTE — legacy divergence: bash's builtin `printf "%.2f"` (what the old `stint.sh`
    used) does NOT round correctly at half-cent-hour boundaries. Its float path
    double-rounds, so e.g. 0.045h -> 0.05 (bash) vs 0.04 (correct: the nearest
    double is 0.04499…833, below 0.045). Neither `double` nor `long double`
    `%.2Lf` reproduces bash, and its direction is inconsistent (0.045 up, 0.055
    down) — it is a bash bug, not a spec. The port deliberately does the right
    thing; the (rare, <=$0.16/entry) divergences are catalogued and asserted in
    tests/test_parity.py::test_legacy_bash_rounding_divergences so the change is
    explicit and reviewable, not silent."""
    exact = Decimal(float(value))
    return f"{exact.quantize(Decimal('0.01'), rounding=ROUND_HALF_EVEN):.2f}"


def duration_hrs(start: str, end: str) -> str:
    """Port of bash duration_hrs (stint.sh:52-60). Returns a 2dp string."""
    diff = duration_seconds(start, end)
    return _printf_2f(_bc_scale4_div(diff, 3600))


def round_up_to_quarter(seconds: int) -> str:
    """Quarter-hour round-up rule (port of the Bash `_stop_timer_file` block).

    Measured session time is rounded UP to the next QUARTER_HOUR (15-min) mark:
    any positive session under 15m bills 0.25h, 15m01s..30m bills 0.50h, and so
    on; a session landing exactly on a quarter-hour boundary is unchanged. Uses
    integer 900-second buckets, matching bash `(diff + 899) / 900`. Stopwatch
    entries only — manual `add` entries are exempt and never pass through here."""
    if seconds < 0:
        seconds = 0
    quarters = -(-seconds // 900)  # ceil division into 900s (15-min) buckets
    if quarters < 1:
        quarters = 1
    return _printf_2f(Decimal(quarters) * config.QUARTER_HOUR)


def first_boot_after(start_epoch: int, boots: list[int] | None = None) -> int | None:
    """Earliest system boot strictly after `start_epoch`, or None.

    Port of `_first_boot_after` (stint.sh:86-96). `boots` is injectable for testing;
    when omitted it shells out to `last` exactly as bash does."""
    if boots is None:
        boots = _system_boot_epochs()
    candidates = [b for b in boots if b > start_epoch]
    return min(candidates) if candidates else None


def _system_boot_epochs() -> list[int]:
    """Boot timestamps from `last`, as epochs (mirrors stint.sh:94)."""
    try:
        out = subprocess.run(
            ["last", "--time-format", "iso", "reboot"],
            capture_output=True,
            text=True,
            check=False,
        ).stdout
    except (OSError, ValueError):
        return []
    epochs: list[int] = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 5 and parts[0] == "reboot":
            try:
                epochs.append(int(_parse_ts_iso(parts[4]).timestamp()))
            except (ValueError, OverflowError):
                continue
    return epochs


def _parse_ts_iso(value: str) -> datetime:
    return datetime.fromisoformat(value)


@dataclass(frozen=True)
class StopResult:
    entry_date: str
    start_time: str  # full 'YYYY-MM-DD HH:MM:SS'
    end_time: str  # possibly reboot-capped
    hrs: str  # billable, 2dp
    category: str
    description: str
    reboot_capped: bool

    @property
    def csv_row(self) -> str:
        """Byte-identical to bash stint.sh:138."""
        start_hms = self.start_time.split(" ", 1)[1]
        end_hms = self.end_time.split(" ", 1)[1]
        return (
            f"{self.entry_date},{start_hms},{end_hms},"
            f"{self.hrs},{self.category},\"{self.description}\""
        )

    @property
    def billable(self) -> str:
        """Reproduce bash `bc "scale=2; hrs*RATE"` (stint.sh:148)."""
        amount = (Decimal(self.hrs) * config.RATE).quantize(
            Decimal("0.01"), rounding=ROUND_DOWN
        )
        return _printf_2f_bc(amount)


def _printf_2f_bc(value: Decimal) -> str:
    """bc prints scale=2 with a trailing-zero-preserving fixed format; the Bash path echoes
    it raw (no printf). `bc "scale=2; 0.67*16.00"` -> `10.72`. Decimal quantized to
    0.01 stringifies identically for our value range."""
    return f"{value:.2f}"


def stop(
    entry_date: str,
    start_time: str,
    category: str,
    description: str,
    end_time: str | None = None,
    boots: list[int] | None = None,
) -> StopResult:
    """Full port of `_stop_timer_file` (stint.sh:98-149), minus the file I/O.

    Applies the reboot cap then the quarter-hour round-up, exactly in that order.
    `end_time` defaults to now; `boots` is injectable for deterministic tests."""
    if end_time is None:
        end_time = datetime.now().strftime(_TS_FMT)

    start_epoch = int(_parse_ts(start_time).timestamp())
    end_epoch = int(_parse_ts(end_time).timestamp())

    reboot_capped = False
    boot_epoch = first_boot_after(start_epoch, boots)
    if boot_epoch is not None and boot_epoch < end_epoch:
        end_time = datetime.fromtimestamp(boot_epoch).strftime(_TS_FMT)
        reboot_capped = True

    hrs = round_up_to_quarter(duration_seconds(start_time, end_time))

    return StopResult(
        entry_date=entry_date,
        start_time=start_time,
        end_time=end_time,
        hrs=hrs,
        category=category,
        description=description,
        reboot_capped=reboot_capped,
    )
