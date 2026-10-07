"""Single source of truth for rate, categories, clients, and paths.

Today these constants are duplicated (RATE in `stint.sh:6` AND
`legacy-consolidate.py:17`) and the client counters are only half-wired
(`.invoice-counter` is read by the invoicer, `.invoice-counter-kd` is not).
Everything the engine, CLI, and TUI need lives here instead.
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from decimal import Decimal
from pathlib import Path

# Repo root = parent of this package.
_REPO_ROOT = Path(__file__).resolve().parent.parent

# Demo mode: STINT_DEMO=1 points every path at the committed, synthetic `samples/`
# dataset instead of the real (git-ignored) billing CSVs. Lets the TUI be
# launched, screenshotted, or shown off with no real client data present, and
# never touches the real ledger. Read once at import — set the env before launch.
# `SAVD_DEMO` is the pre-rename spelling, still honoured.
DEMO = os.environ.get("STINT_DEMO", os.environ.get("SAVD_DEMO", "")) == "1"

LOG_DIR = _REPO_ROOT / "samples" if DEMO else _REPO_ROOT
TIMERS_DIR = LOG_DIR / ".timers"

# Billable rate. One place, one value.
RATE = Decimal("16.00")

# Billing granularity: measured stopwatch time is rounded UP to the next
# QUARTER_HOUR (15-min) mark. Any positive session under 15m bills a full
# quarter; longer ones round up to the next quarter. Sessions landing exactly on
# a quarter-hour boundary are unchanged. Manual `add` entries are exempt.
QUARTER_HOUR = Decimal("0.25")

# Invoice line-item rounding granularity.
INVOICE_ROUNDING = Decimal("0.25")

# Monthly ledger filenames are `<CSV_PREFIX>YYYY-MM.csv`. The tool was once
# named after a client, so months written before the rename carry
# LEGACY_CSV_PREFIX; every reader accepts both and a month that already exists
# under the legacy name keeps it, so nothing needs migrating.
CSV_PREFIX = "stint-"
LEGACY_CSV_PREFIX = "savd-"

CSV_HEADER = "date,start_time,end_time,duration_hrs,category,description"

# Ordered so help text / dropdowns are stable.
CATEGORIES: tuple[str, ...] = (
    "pr",
    "async",
    "standup",
    "devops",
    "research",
    "dev",
    "admin",
    "docs",
    "planning",
)

# Human labels used on invoices (was CAT_LABELS in legacy-consolidate.py:19).
CATEGORY_LABELS: dict[str, str] = {
    "pr": "Pull Request Reviews",
    "async": "Async Communication",
    "standup": "Standups",
    "devops": "DevOps",
    "research": "Research",
    "dev": "Development",
    "admin": "Administrative",
    "docs": "Documentation",
    "planning": "Planning",
}


@dataclass(frozen=True)
class Client:
    """A billable client.

    `counter_file` holds the next invoice number. The header fields
    (`legal_name`, `address`, `project_title`) feed the "Bill To" block and
    project title of the generated HTML invoice.

    `html_capable` gates automated HTML generation: a flat-rate USD client
    renders through `stintcore.invoice`. A client whose invoices need a
    different currency, withholding tax or payment block stays hand-built:
    the tool only *reserves the invoice number* and refuses to render HTML."""

    key: str
    display_name: str
    counter_file: str
    counter_seed: int
    rate: Decimal = RATE
    currency: str = "USD"
    legal_name: str = ""
    address: tuple[str, ...] = ()
    project_title: str = ""
    html_capable: bool = True

    @property
    def counter_path(self) -> Path:
        return LOG_DIR / self.counter_file


# Real client identities are personal data and live OUTSIDE the tracked tree, in
# `<LOG_DIR>/clients.json` (git-ignored). The Rust core reads the same file.
# Without one, these placeholders keep every front-end and test working.
CLIENTS_FILE = LOG_DIR / "clients.json"

_DEFAULT_CLIENTS: tuple[Client, ...] = (
    Client(
        key="acme",
        display_name="Acme Corp",
        counter_file=".invoice-counter",
        counter_seed=1001,
        currency="USD",
        legal_name="Acme Corp LLC",
        address=("123 Example St", "Springfield, ST 00000", "USA"),
        project_title="Software Development",
        html_capable=True,
    ),
    Client(
        key="globex",
        display_name="Globex Ltd",
        counter_file=".invoice-counter-globex",
        counter_seed=2001,
        currency="PHP",
        legal_name="Globex Ltd",
        address=("1 Example Ave", "Example City 1000", "PH"),
        project_title="",
        html_capable=False,  # hand-built invoice: number reserved only
    ),
)


def _client_from_json(d: dict) -> Client:
    return Client(
        key=str(d["key"]),
        display_name=str(d.get("display_name", d["key"])),
        counter_file=str(d.get("counter_file", f".invoice-counter-{d['key']}")),
        counter_seed=int(d.get("counter_seed", 1001)),
        rate=RATE,
        currency=str(d.get("currency", "USD")),
        legal_name=str(d.get("legal_name", "")),
        address=tuple(str(a) for a in d.get("address", ())),
        project_title=str(d.get("project_title", "")),
        html_capable=bool(d.get("html_capable", True)),
    )


def load_clients(path: Path = CLIENTS_FILE) -> tuple[dict[str, Client], str]:
    """(clients by key, default key). Format:

        {"default_client": "acme",
         "clients": [{"key": "acme", "display_name": "...", "counter_file": ".invoice-counter",
                      "counter_seed": 1001, "currency": "USD", "legal_name": "...",
                      "address": ["...", "..."], "project_title": "...", "html_capable": true}]}

    A missing or unreadable file falls back to the placeholders; a malformed
    one raises so a typo can't silently invoice the wrong client."""
    if not path.exists():
        clients = {c.key: c for c in _DEFAULT_CLIENTS}
        return clients, _DEFAULT_CLIENTS[0].key
    import json

    data = json.loads(path.read_text(encoding="utf-8"))
    entries = [_client_from_json(d) for d in data.get("clients", [])]
    if not entries:
        raise ValueError(f"{path}: no clients defined")
    clients = {c.key: c for c in entries}
    default = str(data.get("default_client", entries[0].key))
    if default not in clients:
        raise ValueError(f"{path}: default_client {default!r} is not one of {sorted(clients)}")
    return clients, default


CLIENTS, DEFAULT_CLIENT = load_clients()
DEFAULT_CATEGORY = "dev"


def manual_client() -> Client | None:
    """The first client whose invoices are hand-built (`html_capable=False`)."""
    return next((c for c in CLIENTS.values() if not c.html_capable), None)


def is_category(token: str) -> bool:
    return token in CATEGORIES
