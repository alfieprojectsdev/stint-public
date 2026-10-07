"""Read/track layer over the on-disk state (CSVs + .timer files).

The engine (engine.py) computes billing; this module is the I/O around it:
  * read the monthly billable CSVs into `Entry` rows,
  * read the `.timers/*.timer` running-timer files into `RunningTimer` objects,
  * start a new timer and stop a running one.

The write paths (`start_timer`, `stop_timer`) mirror the bash `stint.sh` script
exactly — `stop_timer` delegates the billing to `engine.stop()` (whose
`csv_row` is byte-identical to bash `_stop_timer_file`, enforced by
tests/test_parity.py), then appends that row and removes the timer file. No
billing arithmetic lives here, so there is no second place for it to drift.
"""

from __future__ import annotations

import csv
import os
import tempfile
from dataclasses import dataclass
from datetime import date, datetime, timedelta
from decimal import Decimal, ROUND_DOWN
from pathlib import Path

from . import config, engine

_TS_FMT = "%Y-%m-%d %H:%M:%S"
_MANUAL = "manual"


# ── Entries (logged CSV rows) ───────────────────────────────────────────────


@dataclass(frozen=True)
class Entry:
    """One logged row from a `stint-YYYY-MM.csv` file."""

    entry_date: str  # YYYY-MM-DD
    start_time: str  # HH:MM:SS or "manual"
    end_time: str  # HH:MM:SS or "manual"
    hrs: str  # as stored (e.g. "0.67", "0.5")
    category: str
    description: str

    @property
    def is_manual(self) -> bool:
        return self.start_time == _MANUAL

    @property
    def date(self) -> date:
        return datetime.strptime(self.entry_date, "%Y-%m-%d").date()

    @property
    def hours(self) -> Decimal:
        try:
            return Decimal(self.hrs)
        except Exception:
            return Decimal(0)

    @property
    def amount(self) -> Decimal:
        """Billable $ for this row: hrs * RATE, truncated to cents (bash parity)."""
        return (self.hours * config.RATE).quantize(Decimal("0.01"), rounding=ROUND_DOWN)


def csv_path(year: int, month: int) -> Path:
    """The month's CSV. A month already stored under the legacy prefix keeps
    that file; anything new gets `stint-YYYY-MM.csv`. See config.CSV_PREFIX."""
    current = config.LOG_DIR / f"{config.CSV_PREFIX}{year:04d}-{month:02d}.csv"
    if current.exists():
        return current
    legacy = config.LOG_DIR / f"{config.LEGACY_CSV_PREFIX}{year:04d}-{month:02d}.csv"
    return legacy if legacy.exists() else current


def ensure_header(path: Path) -> None:
    """Write the CSV header if the file does not yet exist (bash `ensure_header`)."""
    if not path.exists():
        path.write_text(config.CSV_HEADER + "\n")


def read_month(year: int, month: int) -> list[Entry]:
    """Parse a monthly CSV into Entry rows. Missing file → empty list."""
    path = csv_path(year, month)
    if not path.exists():
        return []
    entries: list[Entry] = []
    with path.open(newline="") as fh:
        reader = csv.reader(fh)
        for row in reader:
            if not row or row[0] == "date":
                continue  # header / blank
            if len(row) < 6:
                continue  # malformed — skip rather than crash the view
            # csv.reader already strips the quoting on the description; a
            # description containing commas is rejoined here defensively.
            entry_date, start, end, hrs, category = row[:5]
            description = ",".join(row[5:])
            entries.append(
                Entry(entry_date.strip(), start.strip(), end.strip(),
                      hrs.strip(), category.strip(), description)
            )
    return entries


def available_months() -> list[tuple[int, int]]:
    """All (year, month) pairs with a CSV on disk, newest first."""
    months: list[tuple[int, int]] = []
    for prefix in (config.CSV_PREFIX, config.LEGACY_CSV_PREFIX):
        for p in config.LOG_DIR.glob(f"{prefix}*.csv"):
            parts = p.stem.split("-")
            if len(parts) == 3 and parts[1].isdigit() and parts[2].isdigit():
                months.append((int(parts[1]), int(parts[2])))
    return sorted(set(months), reverse=True)


# ── Totals ──────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Totals:
    hours: Decimal
    amount: Decimal


def _sum(entries: list[Entry]) -> Totals:
    hrs = sum((e.hours for e in entries), Decimal(0))
    amt = sum((e.amount for e in entries), Decimal(0))
    return Totals(hrs, amt)


def summary(today: date | None = None) -> dict[str, Totals]:
    """Today / current-week (Mon–Sun) / current-month totals of *logged* entries.

    Reads the current and previous month CSVs so a week straddling a month
    boundary is still summed correctly."""
    if today is None:
        today = datetime.now().date()

    cur = read_month(today.year, today.month)
    prev_year, prev_month = (today.year, today.month - 1) if today.month > 1 else (today.year - 1, 12)
    window = cur + read_month(prev_year, prev_month)

    week_start = today - timedelta(days=today.weekday())  # Monday
    week_end = week_start + timedelta(days=6)

    return {
        "today": _sum([e for e in window if e.date == today]),
        "week": _sum([e for e in window if week_start <= e.date <= week_end]),
        "month": _sum(cur),
    }


# ── Running timers (.timer files) ────────────────────────────────────────────


@dataclass(frozen=True)
class RunningTimer:
    timer_id: int
    entry_date: str
    start: str  # 'YYYY-MM-DD HH:MM:SS'
    category: str
    description: str
    path: Path

    def elapsed_seconds(self, now: datetime | None = None) -> int:
        if now is None:
            now = datetime.now()
        try:
            start_dt = datetime.strptime(self.start, _TS_FMT)
        except ValueError:
            return 0
        return max(0, int((now - start_dt).total_seconds()))

    def elapsed_hms(self, now: datetime | None = None) -> str:
        secs = self.elapsed_seconds(now)
        return f"{secs // 3600:02d}:{(secs % 3600) // 60:02d}:{secs % 60:02d}"

    def live_amount(self, now: datetime | None = None) -> Decimal:
        """Indicative $ so far (raw elapsed × rate, no quarter-hour round-up)."""
        hrs = Decimal(self.elapsed_seconds(now)) / Decimal(3600)
        return (hrs * config.RATE).quantize(Decimal("0.01"), rounding=ROUND_DOWN)


def _read_timer_file(path: Path) -> dict[str, str]:
    data: dict[str, str] = {}
    for line in path.read_text().splitlines():
        if "=" in line:
            key, _, val = line.partition("=")
            data[key] = val
    return data


def read_timers() -> list[RunningTimer]:
    """All running timers, ordered by id (matches `stint.sh status`)."""
    timers: list[RunningTimer] = []
    if not config.TIMERS_DIR.exists():
        return timers
    for path in config.TIMERS_DIR.glob("*.timer"):
        d = _read_timer_file(path)
        try:
            tid = int(d.get("id", path.stem))
        except ValueError:
            tid = 0
        timers.append(
            RunningTimer(
                timer_id=tid,
                entry_date=d.get("date", ""),
                start=d.get("start", ""),
                category=d.get("category", config.DEFAULT_CATEGORY),
                description=d.get("description", ""),
                path=path,
            )
        )
    return sorted(timers, key=lambda t: t.timer_id)


def _next_timer_id() -> int:
    """Lowest free id (bash: `while [[ -f id.timer ]]; do id++`)."""
    i = 1
    while (config.TIMERS_DIR / f"{i}.timer").exists():
        i += 1
    return i


def start_timer(description: str, category: str = config.DEFAULT_CATEGORY) -> RunningTimer:
    """Create a new running timer file (bash `cmd_start`). No CSV write yet."""
    config.TIMERS_DIR.mkdir(parents=True, exist_ok=True)
    tid = _next_timer_id()
    now = datetime.now().strftime(_TS_FMT)
    entry_date = now.split(" ", 1)[0]
    path = config.TIMERS_DIR / f"{tid}.timer"
    path.write_text(
        f"id={tid}\n"
        f"date={entry_date}\n"
        f"start={now}\n"
        f"category={category}\n"
        f"description={description}\n"
    )
    return RunningTimer(tid, entry_date, now, category, description, path)


def _append_row(path: Path, row: str) -> None:
    """Append a CSV row atomically-enough for a single-user append.

    bash uses a plain `>> file`; we open in append mode which is equally atomic
    for a lone writer and avoids clobbering the file."""
    with path.open("a", newline="") as fh:
        fh.write(row + "\n")


def serialize_row(e: Entry) -> str:
    """Serialize an Entry to a CSV line in the ledger's CSV format: bare fields + an
    always-double-quoted description. Unlike the bash path (and engine.csv_row,
    which faithfully reproduces it), embedded double quotes are escaped as `""`
    per RFC 4180 — this fixes the latent corruption bash has for descriptions
    containing a `"` (spec §engine fidelity note). Rows without quotes serialize
    byte-identically to the bash output."""
    desc = e.description.replace('"', '""')
    return f'{e.entry_date},{e.start_time},{e.end_time},{e.hrs},{e.category},"{desc}"'


def write_month(year: int, month: int, entries: list[Entry]) -> Path:
    """Rewrite a monthly CSV atomically (temp file + os.replace) from Entry rows.

    The header is always written first; every row goes through serialize_row so
    quoting is uniform. Atomic replace means a crash mid-write can never leave a
    truncated CSV — the old file survives untouched until the new one is complete."""
    path = csv_path(year, month)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), prefix=f".stint-{year}-{month:02d}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", newline="") as fh:
            fh.write(config.CSV_HEADER + "\n")
            for e in entries:
                fh.write(serialize_row(e) + "\n")
        os.replace(tmp, path)
    except Exception:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
    return path


def add_entry(
    description: str,
    category: str,
    hrs: str,
    entry_date: str | None = None,
) -> Entry:
    """Append a manual entry (bash `cmd_add`): start/end = 'manual', hours verbatim
    (quarter-hour round-up exempt). Rewrites the month atomically."""
    if entry_date is None:
        entry_date = datetime.now().strftime("%Y-%m-%d")
    year, month = int(entry_date.split("-")[0]), int(entry_date.split("-")[1])
    entry = Entry(entry_date, _MANUAL, _MANUAL, hrs, category, description)
    rows = read_month(year, month)
    rows.append(entry)
    write_month(year, month, rows)
    return entry


def update_entry(year: int, month: int, index: int, **fields) -> Entry:
    """Replace the row at `index` (0-based, read_month order) with edited fields.

    Allowed fields: entry_date, start_time, end_time, hrs, category, description.
    Rewrites the month atomically. Raises IndexError if the row is gone."""
    rows = read_month(year, month)
    if not (0 <= index < len(rows)):
        raise IndexError(f"row {index} out of range (month has {len(rows)} rows)")
    old = rows[index]
    allowed = {"entry_date", "start_time", "end_time", "hrs", "category", "description"}
    bad = set(fields) - allowed
    if bad:
        raise ValueError(f"unknown fields: {sorted(bad)}")
    merged = {**old.__dict__, **fields}
    new = Entry(**merged)
    rows[index] = new
    # An edit can move the row to a different month (changed date); handle that.
    new_year, new_month = new.date.year, new.date.month
    if (new_year, new_month) != (year, month):
        rows.pop(index)
        write_month(year, month, rows)
        dest = read_month(new_year, new_month)
        dest.append(new)
        write_month(new_year, new_month, dest)
    else:
        write_month(year, month, rows)
    return new


def delete_entry(year: int, month: int, index: int) -> Entry:
    """Delete the row at `index` (0-based, read_month order). Rewrites atomically."""
    rows = read_month(year, month)
    if not (0 <= index < len(rows)):
        raise IndexError(f"row {index} out of range (month has {len(rows)} rows)")
    removed = rows.pop(index)
    write_month(year, month, rows)
    return removed


def stop_timer(timer: RunningTimer, end_time: str | None = None) -> engine.StopResult:
    """Stop a running timer: reboot-cap + quarter-hour round-up (via engine.stop),
    append the byte-identical CSV row, remove the timer file. Mirrors bash
    `_stop_timer_file` exactly."""
    result = engine.stop(
        entry_date=timer.entry_date,
        start_time=timer.start,
        category=timer.category,
        description=timer.description,
        end_time=end_time,
    )
    year = int(result.entry_date.split("-")[0])
    month = int(result.entry_date.split("-")[1])
    path = csv_path(year, month)
    ensure_header(path)
    _append_row(path, result.csv_row)
    try:
        timer.path.unlink()
    except FileNotFoundError:
        pass
    return result
