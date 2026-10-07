# stint-logs

Personal contractor time-tracking tooling: a Bash CLI (`stint.sh`) that logs
billable work to monthly CSVs, a Python core (`stintcore/`) with the shared
billing rules, and a Textual TUI (`stint.sh tui`) for a track → review →
consolidate → invoice workflow.

> **Private repo.** Versions the **tooling only** — the actual time logs,
> invoice state, and exports are git-ignored (they hold billable amounts and
> client identifiers). See `.gitignore`.

## CLI

```bash
stint.sh start "description (org/<repo>)" <category>       # start a timer (description FIRST)
stint.sh stop [id]                                         # stop + log to stint-YYYY-MM.csv
stint.sh add "description" <category> <hours> [YYYY-MM-DD] # retroactive manual entry
stint.sh status                                            # show running timers
stint.sh tui                                               # full-screen app (see below)
```

## Desktop app (`stint gui`)

`stint` is the Rust binary (`crates/`): the same verbs as `stint.sh`, plus an egui
desktop app and `--json` output on everything. Windows is the primary GUI
target; the same code builds on Linux.

```bash
cargo build --release                       # Windows: self-contained target\release\stint.exe
stint gui                                   # Dashboard / Log / Invoice / Report
stint gui --pane invoice                    # open on a pane; --no-fallback to never drop to the TUI
```

On first run the GUI asks for the ledger folder (a WSL path such as
`\\wsl.localhost\Ubuntu\home\<user>\repos\stint` works) and remembers it.

## Ledger filenames

Monthly files are `stint-YYYY-MM.csv`. The tool used to be named after a
client, so months written before the rename are `savd-YYYY-MM.csv`: all four
front-ends read either, and a month already stored under the old name keeps
it, so nothing needs migrating. `SAVD_LOG_DIR` and `SAVD_DEMO` are still
honoured alongside `STINT_LOG_DIR` and `STINT_DEMO`. If you want `./savd` to
keep working, `ln -s stint.sh savd` in your ledger; that name is git-ignored.

## Your clients and rate

Client identities (legal name, address, currency, invoice counter) are
personal data and are **not** in the tree. Put them in `clients.json` at the
ledger root (git-ignored); both the Python and Rust cores read it:

```json
{
  "default_client": "acme",
  "clients": [
    {"key": "acme", "display_name": "Acme Corp", "counter_file": ".invoice-counter",
     "counter_seed": 1001, "currency": "USD", "legal_name": "Acme Corp LLC",
     "address": ["123 Example St", "Springfield, ST 00000", "USA"],
     "project_title": "Software Development", "html_capable": true},
    {"key": "globex", "display_name": "Globex Ltd", "currency": "PHP", "html_capable": false}
  ]
}
```

Without the file, two placeholder clients (`acme`, `globex`) keep everything
working. Your own name, address and payment details live only in your copy of
`temp/invoice-dynamic.html` (also git-ignored); `samples/temp/` holds a
placeholder template. The hourly rate is the one number still in code
(`RATE` in `stint.sh`, `stintcore/config.py`, `crates/stint-core/src/config.rs`;
the parity oracle assumes it).

## MCP server (`stint mcp`)

```bash
claude mcp add stint -- stint mcp
```

Exposes the ledger to Claude Code as typed tools (`start_timer`, `stop_timer`,
`list_running`, `log_month`, `add_entry`, `summary`, `consolidate_preview`,
`render_invoice`, `ledger_info`) over stdio, wrapping the same core the CLI and
GUI use. See `docs/TRANSITION.md` for the verb-to-tool map.

It reads and writes the same CSVs, `.timers/` files and `temp/staging-*.txt`
as the Bash CLI and the Python TUI, re-reading them every second, so all three
can be used on one ledger at the same time. See `CLAUDE.md` for the layout and
the parity harnesses.

## TUI

```bash
stint.sh tui        # needs uv on PATH; runs the Textual app in stintcore/tui/
stint tui       # same app, launched from the Rust binary (the GUI's fallback)
```

Four panes — **Dashboard** (live timers, quick-start, totals), **Log** (browse /
filter / edit-delete-add entries), **Invoice** (client-aware consolidate +
generate HTML), **Report** (per-category / per-week rollups). `?` for keys,
`:` command palette, `ctrl+t` theme. Entry badges: `✎` manual, `+¼`
minimum-engagement applied, `⚡` reboot-capped.

Python deps via **uv** (`uv sync`); tests with `uv run pytest`.

**Optional narrative polish:** `stint.sh consolidate 2026 04 --polish` rewords the
consolidated line-item narratives through Claude Haiku before writing the staging
file. Wording only — hours and totals are computed first and never sent to the
model, and the CLI re-checks they're unchanged before writing. Needs
`ANTHROPIC_API_KEY`; without it (or on any error) it falls back to unpolished
staging. The default `consolidate` (no flag) stays fully offline.

- **Categories:** `pr async standup devops research dev admin docs planning`
- **Arg order:** `start` and `add` take **description first, category second.**
  `cmd_start` is order-tolerant — if you pass a known category token first it
  auto-corrects — but description-first is canonical.
- **Rate / output:** entries are `date,start_time,end_time,duration_hrs,category,description`
  in `stint-YYYY-MM.csv` (ignored). `legacy-consolidate.py` rolls these up for invoicing.

## Data layout (local, not committed)

- `stint-YYYY-MM.csv` — monthly billable log
- `.invoice-counter`, `*.ods` — invoice state / exports
- `samples/` — fake-data CSV illustrating the format (safe to commit)

## Architecture

Two front-ends over one rule set. The Bash `stint.sh` owns the CLI + timer files;
`stintcore/` holds the shared core — `config.py` (single source of rate /
categories / clients), `engine.py` (billing, byte-parity with Bash), `store.py`
(CSV + `.timer` I/O and edits), `invoice.py` (consolidator), `tui/` (the app).
`tests/` enforces that the Python core and the Bash path stay in agreement.
See `CLAUDE.md` for the full map.

## Demo mode

```bash
stint.sh demo            # or: stint.sh tui --demo   (sets STINT_DEMO=1)
```

Launches the TUI against the committed, synthetic `samples/` dataset instead of
the real (git-ignored) ledger — safe to launch, screenshot, or show off with no
client data present. A `DEMO` banner shows in the header; the panes open on the
newest sample month. Invoices generate into `samples/temp/`, never the real
`temp/`. Set `STINT_DEMO=1` in the environment for the same effect on any path.

## Future (optional, not started)

- **ratatui TUI** in the `stint` binary (`stint tui` currently execs the
  Python TUI).
- **SQLite store** (Phase 2) — cross-month queries + edit history; CSV stays the
  export format.
- **HTML for hand-built clients** — a template for clients whose invoices
  carry withholding tax / another currency (currently the tool only reserves
  the number).
