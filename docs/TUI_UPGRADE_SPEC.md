# stint TUI Upgrade — Design Spec

**Status:** Draft for review
**Date:** 2026-07-08
**Scope:** `stint-logs` repo only (personal contractor tooling)

---

## 1. Goal & Non-Goals

### Goal
Wrap the existing `stint.sh` time tracker in a full-screen terminal UI that presents
a coherent **track → review → consolidate → invoice** workflow, so the tool reads
and behaves like a professional time-tracking/invoicing app instead of a set of
disconnected subcommands. The current CLI stays fully functional for scripting
and muscle memory.

### Non-Goals
- No web UI, no server, no cloud sync. Stays local-first, single-user.
- No change to billing economics — `$16/hr`, reboot-cap, and the sub-0.25h rule
  are preserved exactly (see §4).
- No forced data migration in Phase 1 — CSVs remain the source of truth.
- Not a rewrite for its own sake. The bash CLI's observable behavior is a contract.

---

## 2. Current State (audit)

| Piece | File | Role |
|---|---|---|
| CLI dispatcher + timer engine | `stint.sh` (bash, 565 lines) | start/stop/status/watch/work/log/consolidate/invoice/add |
| Invoice consolidator | `legacy-consolidate.py` (366 lines) | grouping, 0.25h rounding, narratives, staging txt, HTML render |
| Running timers | `.timers/<id>.timer` (key=value files) | survive reboots (files, not processes) |
| Billable log | `stint-YYYY-MM.csv` | `date,start_time,end_time,duration_hrs,category,description` |
| Invoice counters | `.invoice-counter` (main client, 1xxx), `.invoice-counter-<key>` (hand-built client, 2xxx) | per-client sequence |
| HTML invoice template | `temp/invoice-dynamic.html` | localStorage-backed, Print-to-PDF |
| Countdown/elapsed helper | `~/scripts/timer.sh` | called by `watch`/`work` |

### Fault lines the upgrade must address (not just paper over)
1. **Duplicated rate.** `RATE=16.00` is defined in both `stint.sh:6` and
   `legacy-consolidate.py:17`. A TUI that hard-codes it a third time guarantees drift.
   → **Single source of config.**
2. **Billing rules live only in bash.** Reboot-cap (`stint.sh:_stop_timer_file`,
   ~line 116) and the sub-0.25h overhead (`stint.sh:128`) exist only in the shell path.
   Any second entry point (TUI) that stops a timer must reuse the *same* logic,
   not reimplement it.
3. **Multi-client is half-wired.** Two counters exist on disk, but
   `legacy-consolidate.py:213 next_invoice_number()` only reads `.invoice-counter`
   (main client). The second client's invoices are effectively manual. A "professional" flow needs
   client as a first-class dimension.
4. **No edit/delete.** Fixing a mistyped entry means hand-editing the CSV. The
   invoice staging file (`temp/staging-YYYY-MM.txt`) is the only editable surface.
5. **Two languages.** Timer engine in bash, invoicing in Python — no shared model.

---

## 3. Architecture

**Principle: one domain core, two front-ends.** Extract all logic into a Python
package; the TUI and a (thin) CLI both call it. The current bash `stint.sh` becomes a
compatibility shim so existing habits/scripts keep working.

```
stint-logs/
├── stintcore/
│   ├── config.py      # RATE, clients, categories, paths — THE single source
│   ├── model.py       # Entry, Timer, LineItem, Client, Invoice dataclasses
│   ├── engine.py      # start/stop/status; reboot-cap + sub-0.25h rule (ported from bash)
│   ├── store.py       # CSV read/write (atomic), .timer files, counters
│   ├── invoice.py     # consolidate.py logic, refactored (grouping/rounding/HTML)
│   ├── cli.py         # argparse dispatcher — same verbs as today
│   └── tui/           # Textual app (see §5)
├── stint.sh            # shim → `python -m stintcore.cli "$@"`  (keeps `watch`/`work` UX)
├── stint-YYYY-MM.csv   # unchanged format (source of truth, Phase 1)
└── temp/…             # unchanged invoice output
```

Migration is behavior-preserving: `stintcore.engine.stop_timer()` must produce a
CSV row **byte-identical** to today's bash path for the same inputs. That is the
acceptance test for the port (§8).

### Framework: **Textual** (recommended)
- Python already owns the invoice half, so no new language.
- Produces genuinely modern TUIs: panels, live-updating tables, mouse, scrollable
  regions, CSS-like theming, a built-in command palette, and modal dialogs — the
  things that make it *look* like an app, not a script.
- Runs over SSH/tmux; degrades gracefully.

**Alternatives considered:** `Rich` alone (rendering only, no event loop — too
low-level for interactive edit); `dialog`/`whiptail` (stays in bash, but ugly and
no live views); `bubbletea`/`ratatui` (Go/Rust — best-looking, but a third
language and a full rewrite — rejected as overkill for a single-user tool).

---

## 4. Billing rules — preserved verbatim
- Rate: **$16.00/hr** (from `stintcore.config`, read by every path).
- **Reboot cap:** a timer that spans a reboot ends at the first boot after start
  (work stopped when the machine went down). Ported from `_first_boot_after`.
- **Minimum engagement:** a *stopwatch* entry under 0.25h is billed as
  `0.25 + measured`. Entries ≥ 0.25h billed as measured.
- **Manual entries exempt:** `add` (and TUI manual add) write hours verbatim.
- **Invoice rounding:** line-item hours round to nearest 0.25h at consolidation
  (`round_quarter`), independent of the per-entry rule above.

The TUI surfaces these as *visible* annotations (e.g. a "+0.25 min" badge on short
entries, a "⚡ reboot-capped" tag) rather than silent arithmetic — transparency is
part of looking professional.

---

## 5. TUI design

Single app, tabbed panes, persistent header (live clock + today/week/month totals)
and footer (context keybindings + command palette hint). Mouse + keyboard.

### 5.1 Dashboard (landing)
```
┌ stint ───────────────────────────── 16:24  Today 3.2h $51.20 │ Wk 18.7h │ Jul 42.1h $673.60 ┐
│ RUNNING                                                                                     │
│  ▶ #1  [pr]     PR #129 email-verification review (example-repo)        00:41:12   $10.72   │
│  ▶ #2  [async]  Client thread: staging go/no-go                         00:12:47   $ 3.41   │
│                                                                                             │
│ QUICK START   [ description……………………………… ]  ( category ▾ dev )     [ Start ▸ ]              │
│                                                                                             │
│ RECENT (today)                                                                              │
│  15:43 → 16:24  pr        PR #129 review …                    0.67h   $10.72               │
│  12:01 → 13:05  async     Reply client: staging readiness      1.07h   $17.12   ⚡capped     │
│  manual         admin     Prepare June invoice …              0.50h   $ 8.00   ✎manual     │
├─────────────────────────────────────────────────────────────────────────────────────────┤
│ [s]tart [x]stop [w]atch  [L]og  [I]nvoice  [C]lients  [?]help  [q]uit        ⌘ palette: :  │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```
- Running timers tick live (per-second). `x` stops the focused one (confirm modal
  showing the exact rule applied: measured vs +0.25, reboot cap).
- Quick-start bar = `stint.sh start` without leaving the app; category is a dropdown of
  the 9 known categories.
- `w` opens the countdown/elapsed watch view (reuses `timer.sh` semantics inline).

### 5.2 Log / Entries (month browser + editor)
```
┌ Log — 2026-07 ◀ ▶ ─────────────────── filter: [ all ▾ ]  cat:[ ▾ ]  🔍[      ] ──────────┐
│ Date        Start  End    Hrs   Cat       Description                          $          │
│ 2026-07-08  15:43  16:24  0.67  pr        PR #129 review …                     10.72      │
│ 2026-07-01  12:01  13:05  1.07  async     Reply client: staging readiness       17.12  ⚡  │
│ 2026-07-01  —      —      0.50  admin     Prepare June invoice …                8.00  ✎  │
│ …                                                                                         │
├──────────────────────────────────────────────────────────────────────────────────────────┤
│ Month total: 42.10h   $673.60      [a]dd  [e]dit  [d]elete  [enter]detail  [I]→invoice     │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```
- `e` edits a row inline (modal form); `d` deletes with confirm; `a` adds a manual
  entry. All writes rewrite the monthly CSV **atomically** (temp + `os.replace`).
- Filter by category / ticket key; free-text search over descriptions.
- Month navigation (◀ ▶) loads other `stint-YYYY-MM.csv` files.

### 5.3 Invoice / Consolidate (the money screen)
```
┌ Invoice — 2026-07 ── Client: [ Acme Corp  ▾ ]  Next #: 1005  Rate $16 ────────────────────┐
│ Line item                          Hrs    Total   Narrative (editable)                     │
│ PR #129                            0.75   12.00   Email-verification review — diff + …      │
│ Development                        6.25  100.00   T10 scraper impl — normalizer, tests …    │
│ Async Communication                4.00   64.00   Staging readiness, go/no-go, …            │
│ …                                                                                          │
├──────────────────────────────────────────────────────────────────────────────────────────┤
│ Raw CSV:  41.83h ($669.28)     Invoice: 42.00h ($672.00)     Δ +$2.72 (0.25h rounding)     │
│ [e]dit line  [+]add  [-]drop  [↑↓]reorder  [G]enerate HTML → temp/invoice-2026-07.html     │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```
- This is the current `consolidate` preview + staging-file edit, made interactive:
  edit label/hours/narrative in place instead of hand-editing `staging-*.txt`.
  (The staging txt can remain as an export/round-trip format for compatibility.)
- **Client selector** drives which counter is used (`.invoice-counter` vs
  `.invoice-counter-kd`) and the client block on the HTML — closes fault-line #3.
- `G` renders the HTML (same template pipeline), increments the correct counter
  atomically, and shows the output path. Confirm modal before overwrite (as today).
- Delta line makes the rounding explicit — no surprise totals.

### 5.4 Clients / Settings
- Per-client: display name, rate, invoice-number counter + prefix, HTML header
  fields (project title, billing terms).
- Category label editor (currently the `CAT_LABELS` map in `consolidate.py:19`).
- Read/write `stintcore.config` — the one place rate & clients live.

### 5.5 Keybinding language (vim-flavored, discoverable)
`Tab`/`1-4` switch panes · `s` start · `x` stop · `w` watch · `a` add · `e` edit ·
`d` delete · `I` invoice · `G` generate · `?` help overlay · `:` command palette ·
`q` quit. Footer always shows the active-pane bindings; nothing is hidden.

---

## 6. Feature set mapped to the invoicing flow

| Stage | Today | After upgrade |
|---|---|---|
| Track | `stint.sh start/stop` | Dashboard quick-start + live running timers |
| Correct | hand-edit CSV | inline edit/delete/add in Log pane |
| Review | `stint.sh log` | filterable month browser with $ + rule badges |
| Consolidate | `stint.sh consolidate` + edit txt | interactive grouped line-item editor |
| Invoice | `stint.sh invoice --html` | client-aware HTML gen with correct counter |
| Multi-client | manual for the second client | first-class client switch end-to-end |

CLI parity retained: every verb above still works headless
(`stint.sh start …`, `stint.sh invoice 2026 07 --html`) via `stintcore.cli`.

---

## 7. Data model decision (open)

**Recommendation: keep CSV as source of truth in Phase 1.** The TUI edits by
rewriting the monthly CSV atomically. This preserves git-ignore hygiene, keeps the
existing invoice pipeline untouched, and means the port is verifiable against the
current output.

**Phase 2 option:** introduce SQLite (`stint.db`) as the primary store with CSV
*export*. Buys: cheap queries (client/date/ticket rollups), true edit history,
soft-delete, and cross-month reporting — the things a "professional app" implies.
Cost: migration + dual-write during transition. Deferred until the TUI proves out.

---

## 8. Delivery plan (phased, POC-first)

**M0 — Core extraction + parity POC (highest value, de-risks everything)**
- Port timer engine to `stintcore.engine`/`store`; bash `stint.sh` becomes a shim.
- **Acceptance:** golden-file test — for a fixed set of start/stop/reboot/short
  scenarios, `stintcore` emits CSV rows byte-identical to the current bash path;
  rate & rules read from `stintcore.config` only (delete the duplicate constants).
- No UI yet. This alone kills fault-lines #1, #2, #5.

**M1 — Textual TUI, read + track**
- Dashboard (live timers, quick-start, totals) + Log pane (browse/filter, read-only).
- `stint.sh tui` launches it; CLI unchanged.

**M2 — Edit + invoice**
- Inline edit/delete/add (atomic CSV rewrite).
- Interactive consolidate/invoice pane + **client selector** wired to both counters
  (fault-line #3) + HTML generation.

**M3 — Polish**
- Theming, command palette actions, help overlay, rule-badge tooltips, week/month
  reporting view. Optional Phase-2 SQLite spike.

---

## 9. Open decisions for review
1. **Framework:** Textual (recommended) vs staying bash+`dialog`.
2. **Storage:** CSV-as-truth Phase 1 (recommended) vs jump straight to SQLite.
3. **Bash CLI:** keep as a permanent shim (recommended) vs deprecate once TUI lands.
4. **Multi-client scope:** wire both current clients now, or design for N clients from
   the start (config-driven, recommended — marginal extra cost).
5. **`timer.sh` integration:** call out to it as today, or reimplement countdown
   natively inside Textual (nicer, but re-does working code).

---

## 10. Risks
- **Behavior drift during the port** → mitigated by the M0 golden-file parity test.
- **Counter corruption on concurrent invoice runs** → keep the atomic
  temp+`os.replace` pattern already in `commit_invoice_number`.
- **Scope creep into a "real" app** → phased plan; each milestone independently
  useful; SQLite/web explicitly out until earned.
- **Textual learning curve** → contained to `stintcore/tui/`; core + CLI have none.
