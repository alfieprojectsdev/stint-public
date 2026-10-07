# Moving from the `stint.sh` CLI to `stint` (GUI and MCP)

Audience: Claude Code sessions (and humans) that have been driving time tracking
through `./stint.sh start|stop|add|log|consolidate|invoice` and editing the CSVs by
hand. Everything below keeps working; this explains what changed and what to use
instead.

## The short version

- The data did not change. Same `stint-YYYY-MM.csv`, same `.timers/<id>.timer`
  files, same `temp/staging-YYYY-MM.txt`, same counters. `stint.sh`, the Python TUI
  and the new `stint` binary all read and write the same files, so they can be
  mixed freely on one ledger.
- `stint` is a single native binary (Windows `stint.exe`, Linux `stint`) with
  the same verbs as `stint.sh`, plus `--json` on every verb and a desktop GUI
  (`stint gui`). It is on PATH on the Windows box and installed at
  `~/.local/bin/stint` in WSL.
- `stint mcp` exposes the same operations to Claude Code as typed tools. Register
  it once (below) and sessions can stop parsing CLI output; `--json` remains
  available where a shell call is more convenient.
- The GUI is for the human. Sessions should not try to operate it; they change
  the files and the GUI picks the change up within a second.

## Verb map

| You used to run | Run now | Notes |
|---|---|---|
| `./stint.sh start "desc" cat` | `stint start "desc" cat` | Same order-tolerance (`start cat "desc"` also works). Category may not contain `,` `"` or a newline. |
| `./stint.sh stop [id]` | `stint stop [id]` | Same quarter-hour round-up and reboot cap; same explanatory lines in the output. With `--json`, an ambiguous stop returns `{"error":…,"running":[ids]}` and exit 1. |
| `./stint.sh stop-all` | `stint stop-all` | |
| `./stint.sh status` | `stint status` | |
| `./stint.sh log 2026 09` | `stint log 2026 9` | Month accepts `9` or `09`. |
| `./stint.sh add "desc" cat 1.5 [date]` | `stint add "desc" cat 1.5 [date]` | Hours must parse as a non-negative decimal (`1,5` and `abc` are rejected instead of corrupting the row). Date is normalised to `YYYY-MM-DD`. |
| `./stint.sh consolidate 2026 09` | `stint consolidate 2026 9` | Same preview, writes the same `temp/staging-2026-09.txt`. |
| `./stint.sh invoice 2026 09 --html` | `stint invoice 2026 9 --html` | Staged items win when the staging file exists. `--client <key>` for a hand-built client reserves a number only. Keys come from `clients.json`. |
| `./stint.sh consolidate … --polish` | still `./stint.sh consolidate … --polish` | The LLM wording pass lives in the Python core; `stint` has no equivalent yet. |
| `./stint.sh tui` / `./stint.sh watch` / `./stint.sh work` | unchanged | `stint tui` execs the same Python TUI. |
| (new) | `stint summary` | today / week / month totals. |
| (new) | `stint gui [--pane log]` | desktop app; falls back to the TUI without a display. |
| (new) | `stint home` | prints the ledger folder the binary resolved. |

Everything accepts `--json` (before or after the verb) and `--home <dir>`.

## Where the ledger is

`stint` resolves the ledger folder in this order: `--home`, `$STINT_LOG_DIR`
(the same override `stint.sh` honours), `$STINT_HOME`, the folder remembered by the
GUI (`%APPDATA%\stint\home.txt` on Windows, `~/.config/stint/home.txt` on
Linux), then `~/repos/stint`. Empty variables count as unset, like Bash.

On the Windows box the remembered folder is the WSL repo over UNC:
`\\wsl.localhost\<distro>\home\<user>\repos\stint`. So a session running on Windows
can call `stint` directly, without `wsl -d Ubuntu -- ./stint.sh …`, and it writes to
the same files the Linux side sees. Inside WSL, `stint` and `./stint.sh` resolve the
same folder through the `~/repos/stint` symlink.

Check what a session is about to write to before the first write:

```bash
stint home
```

## Using `--json` from a session

Prefer `--json`; the human output is stable but the JSON is the contract.

```bash
stint --json start "T123 fix retry backoff (org/repo)" dev
# {"timer_id":1,"entry_date":"2026-09-21","start":"2026-09-21 09:15:02","category":"dev","description":"…","path":"…"}

stint --json status
# [{"timer_id":1,"elapsed_seconds":312,"elapsed_hms":"00:05:12",…}]

stint --json stop
# {"entry_date":…,"hrs":"0.25","measured_hrs":"0.09","billable":"4.00","reboot_capped":false,"csv_row":"…",…}

stint --json log 2026 9        # array of rows as stored
stint --json summary           # {"today":{"hours":"0.50","amount":"8.00"},"week":…,"month":…}
stint --json consolidate 2026 9   # {"items":[…],"totals":{…},"staging":"…/temp/staging-2026-09.txt"}
stint --json invoice 2026 9       # {"items":[…],"staged":true|false,"totals":{…}}
stint --json invoice 2026 9 --html   # {"invoice_number":1008,"path":"…/temp/invoice-2026-09.html","items":7}
```

Money and hours are strings in JSON (exact decimals), not floats, except the
invoice line items, which mirror the float arithmetic of the legacy consolidator.

The billing rule reminder that `stint.sh stop` prints is still there and still
means what it says: a measured `0.09h` billed as `0.25h` is the quarter-hour
convention, not a bug. Do not "fix" it and do not edit the row down.

## Editing the CSV directly

Still allowed, same rules as before (match the header, quote descriptions with
commas, LF line endings). Two things are different now:

- The GUI re-reads the ledger about once a second and, before applying an edit
  or delete, re-checks that the row it displayed is still the row on disk. If
  a session rewrites a month while the human has a row selected, the GUI
  reloads and asks them to reselect rather than touching the wrong row. Batch
  edits are therefore safe, but do them as one atomic rewrite (temp file +
  rename), the way `stintcore.store.write_month` and `stint` do.
- `stint` and the Python core write rows with RFC 4180 quote escaping (`""`).
  Bash `stint.sh stop` still writes the description unescaped. If a description
  contains a double quote, prefer `stint` or `stint.sh add`'s Python path.

## What the GUI does that the CLI does not

For the human, not for sessions: live timers with the running amount, a stop
confirmation that shows the rule applied (`+¼` round-up, `⚡` reboot cap), a
month browser with search and inline edit/delete, invoice line-item editing
that writes the staging file, HTML generation that advances the counter and
opens the file, and per-category / per-week reports. Anything a session writes
shows up there within a second; nothing in the GUI needs restarting.

If a session needs the human to look at something, say which pane:
`stint gui --pane invoice` opens directly on it.

## MCP

`stint mcp` runs a stdio MCP server. Register it once per machine:

```bash
claude mcp add stint -- stint mcp
```

(`stint` must be on PATH, which it is on the Windows box and in WSL. Put
`--home` before `mcp` to pin a ledger: `claude mcp add stint -- stint --home
"\\wsl.localhost\<distro>\home\<user>\repos\stint" mcp`.)

| Tool | CLI verb | Notes |
|---|---|---|
| `start_timer {description, category?}` | `stint start` | An unknown category is accepted (as the CLI does) and flagged in a `note` field. |
| `stop_timer {id?}` | `stint stop [id]` | With several timers running and no id, the result is an error listing the ids. |
| `list_running {}` | `stint status --json` | adds `elapsed_hms`, `live_amount`. |
| `log_month {year?, month?}` | `stint log --json` | entries as stored + totals. |
| `add_entry {description, category, hours, date?}` | `stint add` | same validation; unknown category flagged in `note`. |
| `summary {}` | `stint summary --json` | |
| `consolidate_preview {year?, month?, overwrite?}` | `stint consolidate --json` | writes the staging file; an existing one is returned unchanged unless `overwrite: true` (the CLI always overwrites). |
| `render_invoice {year?, month?, client?}` | `stint invoice --html --json` | A hand-built client reserves a number only. |
| `ledger_info {}` | `stint home --json` | plus the months on disk. |

Results are the same JSON the CLI's `--json` prints, returned as a text
block. Failures (bad category, no timer running, unknown client) come back as
tool results with `isError: true` and a message, so a session can correct and
retry rather than crash on a protocol error. The server writes nothing to
stdout except protocol messages.

The quarter-hour reminder applies to `stop_timer` exactly as it does to
`stint stop`: `hrs` above `measured_hrs` is the billing convention, not a bug.

## Things that stay Linux-only

- The reboot cap on Linux uses `last`; the Windows binary uses the Windows boot
  time instead (a Windows reboot also stops WSL, so the cap still fires).
- `stint.sh watch` and `stint.sh work` (the countdown helpers) have no `stint`
  equivalent and depend on `~/scripts/timer.sh`, which is currently missing on
  the live machine.
- `--polish` (Haiku narrative rewrite) is Python-only.
