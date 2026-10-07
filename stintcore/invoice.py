"""Invoice consolidation — ported from `legacy-consolidate.py`.

Byte-parity contract: for the default client this module produces line items,
staging files and HTML byte-identical to the legacy `legacy-consolidate.py` for
the same CSV input (enforced by tests/test_invoice_parity.py). To guarantee
that, the grouping/rounding arithmetic is kept in `float` exactly as the legacy
script did — the rate is *read* from `config` (killing the third RATE
duplication, config.py comment) but coerced to float so the math is identical.

Differences from legacy, all deliberate:
  * The invoice number counter is selected per client (`config.CLIENTS`) instead
    of hard-coding `.invoice-counter` — closes the half-wired counter (spec
    fault-line #3). Default-client behaviour is unchanged.
  * `render_html` takes an explicit `inv_num` and never touches the counter; the
    caller reads/commits it. Legacy folded both together and was untestable
    without mutating on-disk state.
  * A `html_capable=False` client can reserve a number but raises on HTML
    render — its invoice is hand-built.

Historical short entries: entries are summed AS STORED. The quarter-hour
round-up is a stop-time rule (engine.py); it is never applied retroactively at
consolidation, so regenerating a past month never silently inflates its hours.
"""

from __future__ import annotations

import csv
import os
import re
import tempfile
from collections import OrderedDict
from dataclasses import dataclass
from datetime import date, timedelta
from pathlib import Path

from . import config

# Float, to preserve byte-parity with the legacy float arithmetic.
RATE = float(config.RATE)
LOG_DIR = config.LOG_DIR
TEMPLATE_FILE = LOG_DIR / "temp" / "invoice-dynamic.html"
CAT_LABELS = config.CATEGORY_LABELS


@dataclass
class LineItem:
    label: str
    narrative: str
    hours: float
    rate: float = RATE

    @property
    def total(self) -> float:
        return self.hours * self.rate


# ── Load ─────────────────────────────────────────────────────────────────────


def load_entries(year: int, month: int) -> list[dict]:
    csv_path = LOG_DIR / f"{config.CSV_PREFIX}{year}-{month:02d}.csv"
    if not csv_path.exists():
        legacy = LOG_DIR / f"{config.LEGACY_CSV_PREFIX}{year}-{month:02d}.csv"
        if legacy.exists():
            csv_path = legacy
    if not csv_path.exists():
        raise FileNotFoundError(f"No log file found: {csv_path}")
    entries: list[dict] = []
    with open(csv_path, newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            try:
                hrs = float(row["duration_hrs"])
            except (ValueError, KeyError, TypeError):
                hrs = 0.0
            entries.append(
                {
                    "date": row.get("date", "").strip(),
                    "start_time": row.get("start_time", "").strip(),
                    "end_time": row.get("end_time", "").strip(),
                    "hrs": hrs,
                    "category": row.get("category", "").strip(),
                    "description": row.get("description", "").strip(),
                }
            )
    return entries


# ── Grouping (verbatim port) ──────────────────────────────────────────────────


def extract_group_key(description: str) -> str | None:
    matches = []
    for m in re.finditer(r"PR\s+#(\d+)", description, re.IGNORECASE):
        matches.append((m.start(), f"PR #{m.group(1)}"))
    for m in re.finditer(r"issue\s+#(\d+)", description, re.IGNORECASE):
        matches.append((m.start(), f"issue #{m.group(1)}"))
    for m in re.finditer(r"\bT(\d+)\b", description):
        matches.append((m.start(), f"T{m.group(1)}"))
    if not matches:
        return None
    return min(matches, key=lambda x: x[0])[1]


def group_entries(entries: list[dict]) -> "OrderedDict":
    ticket_groups: OrderedDict = OrderedDict()
    cat_groups: OrderedDict = OrderedDict()
    for entry in entries:
        key = extract_group_key(entry["description"])
        if key:
            ticket_groups.setdefault(("ticket", key), []).append(entry)
        else:
            cat_groups.setdefault(("cat", entry["category"]), []).append(entry)
    result: OrderedDict = OrderedDict()
    result.update(ticket_groups)
    result.update(cat_groups)
    return result


def round_quarter(hrs: float) -> float:
    return round(hrs * 4) / 4


def _em_dash_phrase(description: str) -> str | None:
    parts = re.split(r"\s*[—–]\s*", description, maxsplit=1)
    return parts[1].strip() if len(parts) > 1 else None


def build_line_item(label_type: str, label_key: str, entries: list[dict]) -> LineItem:
    if label_type == "ticket":
        display_label = label_key
    else:
        display_label = CAT_LABELS.get(label_key, label_key.title())

    sorted_entries = sorted(entries, key=lambda e: len(e["description"]), reverse=True)
    narrative = sorted_entries[0]["description"]

    em_phrases: list[str] = []
    for e in sorted_entries[1:]:
        phrase = _em_dash_phrase(e["description"])
        if phrase and phrase not in em_phrases and phrase not in narrative:
            em_phrases.append(phrase)
        if len(em_phrases) >= 2:
            break

    if em_phrases:
        narrative = narrative + " — " + ", ".join(em_phrases)

    hours = round_quarter(sum(e["hrs"] for e in entries))
    return LineItem(label=display_label, narrative=narrative, hours=hours, rate=RATE)


def line_items_for(year: int, month: int) -> tuple[list[LineItem], float]:
    """Grouped line items + the raw (unrounded) total hours for a month."""
    entries = load_entries(year, month)
    groups = group_entries(entries)
    items = [build_line_item(lt, lk, ents) for (lt, lk), ents in groups.items()]
    raw_total = sum(e["hrs"] for e in entries)
    return items, raw_total


# ── Staging (verbatim port) ───────────────────────────────────────────────────


def staging_path(year: int, month: int) -> Path:
    return LOG_DIR / "temp" / f"staging-{year}-{month:02d}.txt"


def write_staging(line_items: list[LineItem], year: int, month: int, today: date) -> Path:
    path = staging_path(year, month)
    lines = [
        f"# stint.sh invoice staging — {year}-{month:02d}",
        f"# Generated: {today}",
        "# Edit label, hours, and narrative. Delete rows to exclude.",
        "# Rate is fixed at $16.00/hr.",
        "#",
        "# label | hours | narrative",
        "#",
    ]
    for item in line_items:
        lines.append(f"{item.label} | {item.hours:.2f} | {item.narrative}")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def read_staging(year: int, month: int) -> list[LineItem] | None:
    path = staging_path(year, month)
    if not path.exists():
        return None
    items: list[LineItem] = []
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(" | ", 2)
        if len(parts) < 3:
            continue
        try:
            hours = float(parts[1].strip())
        except ValueError:
            continue
        items.append(LineItem(label=parts[0].strip(), narrative=parts[2].strip(), hours=hours))
    return items if items else None


# ── Counter (per-client) ──────────────────────────────────────────────────────


def next_invoice_number(client_key: str = config.DEFAULT_CLIENT) -> int:
    client = config.CLIENTS[client_key]
    path = client.counter_path
    if not path.exists():
        path.write_text(f"{client.counter_seed}\n")
        return client.counter_seed
    return int(path.read_text().strip())


def commit_invoice_number(current: int, client_key: str = config.DEFAULT_CLIENT) -> None:
    path = config.CLIENTS[client_key].counter_path
    fd, tmp_path = tempfile.mkstemp(dir=str(path.parent))
    try:
        with os.fdopen(fd, "w") as f:
            f.write(f"{current + 1}\n")
        os.replace(tmp_path, path)
    except Exception:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass
        raise


# ── HTML render (verbatim port + client header) ───────────────────────────────


def _last_day_of_month(year: int, month: int) -> date:
    if month == 12:
        return date(year, 12, 31)
    return date(year, month + 1, 1) - timedelta(days=1)


def _fmt_date(d: date) -> str:
    # `%-d` is glibc-only; build the unpadded day by hand so Windows agrees.
    return f"{d.day} {d.strftime('%B %Y')}"


def _html_escape(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _js_escape(text: str) -> str:
    return text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "")


def build_html(
    line_items: list[LineItem],
    entries: list[dict],
    year: int,
    month: int,
    inv_num: int,
    client_key: str = config.DEFAULT_CLIENT,
) -> str:
    """Pure HTML string builder — no counter side effects, no file I/O.

    Byte-identical to legacy `render_html` for the default client (which is the
    only client the legacy path ever produced). Additionally rewrites the
    Bill-To block + project title from `config.CLIENTS[client_key]`."""
    client = config.CLIENTS[client_key]
    if not client.html_capable:
        raise ValueError(
            f"Client '{client_key}' ({client.display_name}) is not HTML-capable — "
            f"its invoices ({client.currency}, withholding tax) are hand-built. "
            f"Reserve the number with next_invoice_number('{client_key}') and build the HTML manually."
        )

    issue_date = _last_day_of_month(year, month)
    due_date = issue_date + timedelta(days=15)

    real_dates = [e["date"] for e in entries if e["date"] not in ("", "manual")]
    if real_dates:
        min_d = date.fromisoformat(min(real_dates))
        max_d = date.fromisoformat(max(real_dates))
    else:
        min_d = date(year, month, 1)
        max_d = issue_date
    billing_period = f"Billing Period: {_fmt_date(min_d)} - {_fmt_date(max_d)}"

    template = TEMPLATE_FILE.read_text(encoding="utf-8")

    js_rows = []
    for item in line_items:
        label_html = f"<strong>{_html_escape(item.label)}:</strong>"
        desc_html = f"{label_html} {_html_escape(item.narrative)}"
        js_rows.append(
            f'            {{ desc: "{_js_escape(desc_html)}",'
            f" hours: {item.hours:.2f}, rate: {item.rate:.2f} }}"
        )
    js_array = "[\n" + ",\n".join(js_rows) + "\n        ]"

    template = re.sub(
        r"const defaultItems = \[.*?\];",
        f"const defaultItems = {js_array};",
        template,
        flags=re.DOTALL,
    )
    template = re.sub(
        r"const STORAGE_KEY = '[^']*';",
        f"const STORAGE_KEY = 'stint_invoice_{year}_{month:02d}';",
        template,
        count=1,
    )
    template = re.sub(
        r'(<span id="invoice-number"[^>]*>)[^<]*(</span>)',
        rf"\g<1>{inv_num}\g<2>",
        template,
    )
    template = re.sub(
        r'(<span id="issue-date"[^>]*>)[^<]*(</span>)',
        rf"\g<1>{_fmt_date(issue_date)}\g<2>",
        template,
    )
    template = re.sub(
        r'(<span id="due-date"[^>]*>)[^<]*(</span>)',
        rf"\g<1>{_fmt_date(due_date)}\g<2>",
        template,
    )
    project_title = client.project_title or "Full-Stack Software Architecture & IT Development"
    template = re.sub(
        r'(<h3 id="project-title"[^>]*>)[^<]*(</h3>)',
        rf"\g<1>{_html_escape(project_title)}\g<2>",
        template,
    )
    template = re.sub(
        r'(<p id="billing-period"[^>]*>)[^<]*(</p>)',
        rf"\g<1>{billing_period}\g<2>",
        template,
    )
    # Client "Bill To" block (legacy left these as the template's own defaults;
    # we now drive them from config so a second client renders correctly).
    if client.legal_name:
        template = re.sub(
            r'(<p id="client-name"[^>]*>)[^<]*(</p>)',
            rf"\g<1>{_html_escape(client.legal_name)}\g<2>",
            template,
        )
    for i, line in enumerate(client.address[:3], start=1):
        template = re.sub(
            rf'(<p id="client-addr-{i}"[^>]*>)[^<]*(</p>)',
            rf"\g<1>{_html_escape(line)}\g<2>",
            template,
        )
    return template


def out_path_for(year: int, month: int) -> Path:
    return LOG_DIR / "temp" / f"invoice-{year}-{month:02d}.html"
