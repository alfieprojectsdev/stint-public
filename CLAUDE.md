# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`stint.sh` is a personal time-tracker and invoice generator for billing freelance work at a flat $16.00/hr. It began as a single Bash script; it is now **Bash CLI + a Python core (`stintcore/`) + a Textual TUI**, in a deliberate layering:

- **`stint.sh`** (Bash) — the CLI dispatcher and the timer-file engine. Still the canonical entry point; `start/stop/status/watch/work/log/add` run entirely in Bash.
- **`stintcore/`** (Python package) — the extracted domain core. `config.py` is the **single source** of rate, categories, and clients; `engine.py` ports the timer/billing rules with byte-parity to the Bash path; `store.py` reads/writes CSVs and `.timer` files; `invoice.py` ports the consolidator; `tui/` is the Textual app.
- **`stint.sh tui`** — launches the full-screen app (Dashboard / Log / Invoice / Report).

The Bash CLI and the Python core are **two front-ends over the same rules**. When changing billing behavior, change it in `stintcore/config.py` + `engine.py` and keep the Bash path in sync — the parity tests (below) enforce that the two agree.

## Rust binary (`crates/`): CLI + GUI

The GUI upgrade is a Rust port of the core so one small binary (`stint`, ~5 MB release) ships the CLI, the egui desktop app and an MCP server (a ratatui TUI is still pending). Layout:

- `crates/stint-core/` — library: `config` (rate/clients/`Home` ledger resolution), `engine` (stop-time billing, same contract as `stintcore.engine`), `store` (CSV + `.timer` I/O, edits, totals), `invoice` (consolidator, byte-parity with `stintcore.invoice`), `report` (badges, per-category / per-week rollups).
- `crates/stint/` — the `stint` binary: `src/main.rs` (CLI), `src/gui.rs` (eframe app, `gui` feature) and `src/mcp.rs` (stdio MCP server via `rmcp`, `mcp` feature); both features are on by default.

```bash
stint start "desc" [cat] · stop [id] · stop-all · status · log [Y M] · add "desc" cat hrs [date]
stint summary · consolidate [Y M] · invoice [Y M] [--html] [--client <key>]
stint gui [--no-fallback]      # desktop app; falls back to `stint tui` without a display
stint tui                      # execs the Python Textual TUI via uv (ratatui port pending)
stint mcp                      # stdio MCP server; register once: claude mcp add stint -- stint mcp
stint --json <verb>            # machine-readable output on every verb
```

**MCP** (`stint mcp`): tools `start_timer`, `stop_timer`, `list_running`, `log_month`, `add_entry`, `summary`, `consolidate_preview`, `render_invoice`, `ledger_info`. Each wraps the same `stint-core` call as the CLI verb and returns the verb's `--json` shape as a text block; validation failures come back as tool results with `isError` (so the model can recover), not protocol errors. Human output goes to stderr only; stdout is the protocol channel. `tests/test_mcp.py` drives it with raw JSON-RPC.

Ledger resolution: `--home`, then `$STINT_LOG_DIR` (the Bash script's own override), `$STINT_HOME`, then the folder remembered by the GUI's ledger picker (`%APPDATA%\stint\home.txt` / `~/.config/stint/home.txt`), then `~/repos/stint`; empty env vars count as unset like Bash. On a Windows box that keeps the ledger in WSL, the remembered folder is the repo over UNC (`\\wsl.localhost\<distro>\home\<user>\repos\stint`). `STINT_DEMO=1` redirects to `<home>/samples`. All writers emit LF only. `start`/`add` reject a category containing `,` `"` or a newline and a description containing a newline (either would corrupt the row / timer file).

**GUI** (`stint gui`): Dashboard (quick-start, running timers with live elapsed/$, Stop → confirm modal showing the billing rule applied, totals, recent entries with badges), Log (month browser, category filter, search, add/edit/delete with validation; double-click edits), Invoice (client selector, auto or staged line items, edit/drop → `temp/staging-YYYY-MM.txt`, reset to auto, raw-vs-invoice Δ, Generate HTML commits the counter and opens the file; a hand-built client only reserves a number), Report (per-category and per-week tables with right-aligned figures, plus activity heatmaps: time-of-day and day-of-week for the month, and a GitHub-style calendar for the year; `report::hour_of_day_hours` spreads each stopwatch session's billed hours over the clock hours it covered). `ctrl+1..4` panes, `ctrl+t` theme. The ledger is re-read every second, so CLI / Python TUI / Claude Code writes show up live with no daemon. On the live machine it runs under WSLg (`stint gui` from an Ubuntu shell); a native `stint.exe` is the same code built with the MSVC toolchain.

**Windows is the primary GUI target.** `cargo build --release` on Windows (gnullvm toolchain + llvm-mingw on PATH, see `.cargo/config.toml`) gives a self-contained `stint.exe`; the installed copy lives at `%LOCALAPPDATA%\Programs\stint\stint.exe`. The Linux build is the same code (`stint gui` forces Mesa software GL under WSLg).

**Build / test.** Rust tests run in WSL (Windows Application Control on the dev box intermittently blocks freshly built test binaries):

```bash
cargo build --release && install -m755 target/release/stint ~/.local/bin/   # deploy
cargo test                                # core unit tests + headless GUI smoke tests (egui renders without a window)
uv run pytest -q                          # whole suite incl. the Rust parity harnesses below
```

Parity harnesses (pytest, drive `target/{debug,release}/stint` or `$STINT_BIN`; skipped if no binary):
- `tests/test_rust_parity.py` — the `test_parity.py` grid through `stint parity row|hrs|bill`. Live `tests/oracle.sh` on Linux/WSL; `tests/oracle_snapshot.json` elsewhere (regenerate on Linux with `uv run python tests/gen_oracle_snapshot.py` when the grid changes).
- `tests/test_rust_invoice_parity.py` — line items, staging, HTML, staging round-trip and the hand-built-client refusal, byte-identical to `stintcore.invoice`, on `samples/` and on the newest real CSV when present. Fidelity notes live at the top of `crates/stint-core/src/invoice.rs` (CPython's compensated `sum()`, ties-to-even `round()`, `re.sub` template escapes).
- `tests/test_mcp.py` — `stint mcp` over stdio: tool surface, JSON shapes, on-disk effects, error results. The harnesses prefer `target/debug` over `target/release`; set `STINT_BIN` when only one is current.

The Python `stintcore` and the Textual TUI stay as they are: the files are the contract, and all three implementations (Bash, Python, Rust) are tested against the same oracle.

## Environment

Python deps are managed with **uv** (not pip). `pyproject.toml` + `uv.lock` pin `textual`; `.venv/` is local.

```bash
uv sync              # create .venv, install textual + pytest
uv run pytest -q     # run the whole suite (parity + invoice + edit + report)
```

`stint.sh tui` execs `uv run --project "$LOG_DIR" python -m stintcore.tui`, so `uv` must be on `PATH` (`~/.local/bin/uv`).

**Demo mode:** `STINT_DEMO=1` (via `stint.sh demo` / `stint.sh tui --demo`) makes `config.LOG_DIR` point at the committed synthetic `samples/` dataset instead of the real ledger. `config.DEMO` is read once at import — set the env before launch. Everything downstream (`store`, `invoice`, TUI) routes through `config.LOG_DIR`, so demo mode is fully isolated: it reads sample CSVs and writes invoices into `samples/temp/`, never touching real billing data. The `samples/` tree (CSVs + counters + a copy of `invoice-dynamic.html`) is the one financial-shaped data that IS committed — it's fake. Keep it that way.

## Running the tool

```bash
./stint.sh start "description" [category]   # start a timer
./stint.sh stop [id]                        # stop a timer (id required if multiple running)
./stint.sh stop-all                         # stop all running timers
./stint.sh status                           # show running timers
./stint.sh tui                              # full-screen TUI (dashboard/log/invoice/report)
./stint.sh watch [id]                       # live countdown/elapsed (calls ~/scripts/timer.sh)
./stint.sh work <duration> ["desc"] [cat]   # countdown block then auto-log (e.g. 90m, 1h30m)
./stint.sh log [YYYY MM]                    # formatted table for a month
./stint.sh consolidate [YYYY MM] [--polish] # group + round entries, write temp/staging-YYYY-MM.txt
                                        #   --polish: reword narratives via Claude Haiku (wording only)
./stint.sh invoice [YYYY MM] [--html]       # terminal summary (default) or HTML invoice
./stint.sh add "desc" category hours [date] # manual entry
```

Duration formats: `90m` `2h` `1h30m` `45s`

## TUI (`stint.sh tui`)

Four panes (`Tab` / click to switch). Keys are pane-aware; `?` shows the full overlay, `:` or `ctrl+p` opens the command palette, `ctrl+t` toggles light/dark.

| Pane | Does | Keys |
|------|------|------|
| **Dashboard** | Live-ticking running timers, quick-start bar, today/week/month totals | `s` focus quick-start, `enter` start, `x` stop selected (confirm modal shows the billing rule applied) |
| **Log** | Month browser: filter by category, search descriptions, **edit/delete/add** entries | `← →` month, `a` add, `e` edit, `d` delete |
| **Invoice** | Interactive consolidate: client selector, line-item edit/drop, raw-vs-invoice Δ, generate HTML | `← →` month, `e` edit line, `d` drop line, `g` generate |
| **Report** | Per-category and per-week (Mon–Sun) rollups, read-only | `← →` month |

**Badges** (transparency, spec §4): `✎` manual entry · `+¼` rounded up to the next quarter-hour · `⚡` reboot-capped (shown only at stop time — it can't be inferred from a stored row).

**Writes are the only side effects.** The TUI's stop path delegates to `stintcore.engine.stop()` so the appended CSV row is byte-identical to the Bash path. Edit/delete/add rewrite the monthly CSV atomically (temp file + `os.replace`) and escape embedded quotes per RFC 4180.

## Tests / parity contract

`tests/` (pytest, via uv):
- `test_parity.py` — `stintcore.engine` emits CSV rows byte-identical to the Bash `_stop_timer_file` for a grid of scenarios, plus the documented, intentional half-cent-hour rounding divergences.
- `test_invoice_parity.py` — `stintcore.invoice` produces line items, staging, and **HTML byte-identical** to legacy `legacy-consolidate.py` for the default client.
- `test_store_edit.py` — add/update/delete round-trip, atomic-rewrite-on-failure, quote-escaping.
- `test_tui_report.py` — badge inference + report grouping.
- `test_polish.py` — `--polish` rewords narratives only; label/hours/rate/total byte-identical (LLM mocked, no network); prompt never leaks numbers; no `effort`/`thinking` param.

These are the guardrails: any change to billing or invoice output must keep them green (or update them deliberately with a rationale).

## Monthly invoice flow

```bash
stint.sh consolidate 2026 04          # review groupings; writes temp/staging-2026-04.txt
$EDITOR temp/staging-2026-04.txt  # edit labels, hours, narratives; delete rows to exclude
stint.sh invoice 2026 04 --html       # writes temp/invoice-2026-04.html; open and Print to PDF
```

Two equivalent front-ends:
- **Bash CLI:** `stint.sh consolidate` calls `legacy-consolidate.py --mode preview`; `stint.sh invoice --html` calls it with `--mode html`. **The Bash verbs still shell to the legacy `legacy-consolidate.py`** — that script is untouched and remains the CLI's invoice path. The one exception: `stint.sh consolidate --polish` reroutes to `python -m stintcore.consolidate` (stintcore path) for the optional LLM wording pass.
- **TUI:** the Invoice pane uses `stintcore.invoice` (a byte-parity port of the legacy script). Line-item edits write `temp/staging-YYYY-MM.txt`; `g` renders HTML and commits the counter.

Both read `temp/staging-YYYY-MM.txt` when present (falling back to auto-grouped data) and require the template `temp/invoice-dynamic.html`.

The consolidator groups entries by ticket references in descriptions (`T123`, `PR #123`, `issue #123`), leftmost match wins; entries with no ticket fall back to category grouping. Hours round to the nearest 0.25h. Staging format: `label | hours | narrative` (one line per group, `#` = comment).

**`--polish` (optional LLM wording pass, `stintcore/polish.py`):** `stint.sh consolidate 2026 04 --polish` reroutes off the legacy path to `python -m stintcore.consolidate` (via `uv`), which rewords each line item's narrative through **Claude Haiku 4.5** and writes the same `temp/staging-YYYY-MM.txt`. **Financial integrity is structural, not trusted:** grouping/summing/rounding all run in `invoice.py` *before* the model is called; the LLM is handed each item's `label` + source `narrative` only — never hours, rate, or total — so it cannot change a billed amount. The CLI re-asserts `label/hours/rate` are unchanged post-polish before writing (`test_polish.py`). No `effort`/`thinking` param (`effort` 400s on Haiku 4.5). Needs `ANTHROPIC_API_KEY`; on any SDK/network/auth error it prints a warning and writes the **unpolished** deterministic staging (the flag never blocks). Default (no flag) stays fully offline — nothing imports `anthropic`.

**Per-client invoicing** (`stintcore.config.CLIENTS`, loaded from `<LOG_DIR>/clients.json`; the Rust core reads the same file via `Home::new`). Client identities are personal data and stay out of the tree; without the file two placeholders (`acme`, `globex`) are used. Each client has:
- an `html_capable=true` shape (`.invoice-counter`, 1xxx, USD) — fully automated; TUI/GUI generate the HTML;
- or `html_capable=false` (own counter file, its own N-thousand block, e.g. PHP with withholding tax and a different payment block) — **hand-built**; the tool only **reserves the next number** and refuses HTML render (raises `ValueError`). Do not "fix" this into auto-generating a USD-shaped invoice.

Invoice numbers are disjoint per-client blocks, unique across the whole book — never restart-per-client. New client → new N-thousand block + its own `.invoice-counter-<tag>`.

**Billing rule (stop-t`ime):** measured stopwatch time is rounded **UP to the next quarter-hour (0.25h) mark** (`engine.round_up_to_quarter`): any positive session under 15m bills 0.25h, 15m01s–30m bills 0.50h, and so on; a session landing exactly on a quarter-hour boundary is unchanged. Manual `stint.sh add` entries are exempt (hours typed verbatim). It is NOT reapplied retroactively at consolidation — old rows are summed as-stored, so regenerating a past invoice never silently changes its total.

## Data layout

| Path | Purpose |
|------|---------|
| `stint-YYYY-MM.csv` | Completed entries for a month (header: `date,start_time,end_time,duration_hrs,category,description`). Months written before the rename are `savd-YYYY-MM.csv`; every reader accepts both and such a month keeps its name, so a live ledger needs no migration (`config::CSV_PREFIX` / `LEGACY_CSV_PREFIX`, mirrored in Bash `csv_file()` and `stintcore.store.csv_path`) |
| `.timers/<id>.timer` | One file per running timer; key=value format (`id`, `date`, `start`, `category`, `description`) |
| `temp/staging-YYYY-MM.txt` | Editable consolidated line items before HTML generation |
| `temp/invoice-YYYY-MM.html` | Generated invoice (open in browser → Print to PDF) |
| `temp/invoice-dynamic.html` | HTML invoice template (must exist for `--html` to work) |
| `.invoice-counter` | Default client's invoice number seed (auto-incremented) |
| `clients.json` | Client identities + default client (git-ignored; see README) |
| `.invoice-counter-<key>` | Per-client invoice number seed for hand-built clients (2xxx; number reserved only) |
| `stintcore/` | Python core: `config` (rate/clients), `engine` (billing), `store` (I/O + edits), `invoice` (consolidator), `polish` (optional LLM narrative rewrite), `consolidate` (`--polish` CLI), `tui/` (Textual app) |
| `tests/` | pytest parity + edit + report suite (run with `uv run pytest`) |
| `pyproject.toml`, `uv.lock` | uv-managed Python deps (`textual`, `anthropic` for `--polish`) |
| `.archive/` | Old CSV snapshots and one-off working files |
| `tickets/` | Exported ticket zip blocks for reference |
| `gemini-code-*.csv` | Alternative CSV from Gemini (different column layout — no `end_time`) |

## Categories

`pr` · `async` · `standup` · `devops` · `research` · `dev` · `admin` · `docs` · `planning`

## External dependency

`stint.sh watch` and `stint.sh work` delegate to `~/scripts/timer.sh` for the countdown/TUI display. That script is outside this repo.

## Adding manual corrections

Use `stint.sh add` for backdated entries — these write `manual` for both `start_time` and `end_time` in the CSV. When editing a CSV directly, match the existing header exactly and quote descriptions that contain commas.
