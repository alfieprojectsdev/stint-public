"""stint TUI — Dashboard (track), Log (edit), Invoice (money), Report (rollups).

M1 = read + track (dashboard, live timers, quick-start, browse/filter log).
M2 = edit + invoice (inline entry edit/delete/add, interactive consolidate,
client selector, HTML generation). M3 = polish: rule badges, help overlay,
command palette, a Report pane, and theming. Writes go through `store`
(atomic) and billing through the byte-identical `engine` port.
"""

from __future__ import annotations

from collections import OrderedDict
from datetime import datetime, timedelta
from decimal import Decimal

from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.command import DiscoveryHit, Hit, Hits, Provider
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.theme import Theme
from textual.widgets import (
    Button,
    DataTable,
    Footer,
    Input,
    Label,
    Markdown,
    Select,
    Static,
    TabbedContent,
    TabPane,
)

from .. import config, engine, invoice, store

_ALL = "__all__"


def _fmt_money(value) -> str:
    return f"${value:,.2f}"


def _entry_badges(e: store.Entry) -> str:
    """Transparency badges (spec §4): ✎ manual entry, +¼ quarter-hour round-up.

    The +¼ badge is inferred by comparing the stored billable hours against the
    raw measured duration — a stopwatch entry billed above measured means it was
    rounded up to the next quarter-hour mark. Reboot-cap (⚡) can't be inferred
    from a stored row (the capped end looks like any other end) so it's surfaced
    only at stop time in the confirm modal, not here."""
    badges: list[str] = []
    if e.is_manual:
        badges.append("✎")
    else:
        try:
            measured = engine.duration_hrs(
                f"{e.entry_date} {e.start_time}", f"{e.entry_date} {e.end_time}"
            )
            if Decimal(e.hrs) > Decimal(measured):
                badges.append("+¼")
        except Exception:
            pass
    return ("  " + " ".join(badges)) if badges else ""


# A calm, branded theme so the app reads as an app, not a script.
STINT_THEME = Theme(
    name="stint",
    primary="#4a9eff",
    secondary="#8b9dc3",
    accent="#f0a020",
    success="#3fb950",
    warning="#d29922",
    error="#f85149",
    surface="#1c2128",
    panel="#22272e",
    dark=True,
)


class Confirm(ModalScreen[bool]):
    """Generic yes/no confirm."""

    BINDINGS = [Binding("y,enter", "yes", "Yes"), Binding("n,escape", "no", "No")]

    def __init__(self, message: str, yes_label: str = "Yes", yes_variant: str = "error") -> None:
        super().__init__()
        self._message = message
        self._yes_label = yes_label
        self._yes_variant = yes_variant

    def compose(self) -> ComposeResult:
        with Vertical(id="modal"):
            yield Label("Confirm", id="modal-title")
            yield Static(self._message)
            with Horizontal(id="modal-buttons"):
                yield Button(f"{self._yes_label} [y]", variant=self._yes_variant, id="ok")
                yield Button("Cancel [n]", id="cancel")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "ok")

    def action_yes(self) -> None:
        self.dismiss(True)

    def action_no(self) -> None:
        self.dismiss(False)


class EntryForm(ModalScreen[dict | None]):
    """Add or edit a CSV entry. Returns a dict of fields, or None if cancelled."""

    BINDINGS = [Binding("escape", "cancel", "Cancel"), Binding("ctrl+s", "save", "Save")]

    def action_save(self) -> None:
        data = self._collect()
        if data is not None:
            self.dismiss(data)

    def __init__(self, entry: store.Entry | None = None) -> None:
        super().__init__()
        self._entry = entry  # None = add (manual)

    def compose(self) -> ComposeResult:
        e = self._entry
        editing = e is not None
        with Vertical(id="form"):
            yield Label("Edit entry" if editing else "Add manual entry", id="modal-title")
            yield Label("Date (YYYY-MM-DD)")
            yield Input(value=(e.entry_date if e else datetime.now().strftime("%Y-%m-%d")), id="f_date")
            yield Label("Start (HH:MM:SS or 'manual')")
            yield Input(value=(e.start_time if e else "manual"), id="f_start")
            yield Label("End (HH:MM:SS or 'manual')")
            yield Input(value=(e.end_time if e else "manual"), id="f_end")
            yield Label("Hours")
            yield Input(value=(e.hrs if e else "0.25"), id="f_hrs")
            yield Label("Category")
            yield Select(
                [(c, c) for c in config.CATEGORIES],
                value=(e.category if e and e.category in config.CATEGORIES else config.DEFAULT_CATEGORY),
                allow_blank=False,
                id="f_cat",
            )
            yield Label("Description")
            yield Input(value=(e.description if e else ""), id="f_desc")
            with Horizontal(id="modal-buttons"):
                yield Button("Save [ctrl+s]", variant="success", id="ok")
                yield Button("Cancel [esc]", id="cancel")

    def _collect(self) -> dict | None:
        hrs = self.query_one("#f_hrs", Input).value.strip()
        try:
            float(hrs)
        except ValueError:
            self.notify("Hours must be a number.", severity="error")
            return None
        desc = self.query_one("#f_desc", Input).value.strip()
        if not desc:
            self.notify("Description required.", severity="error")
            return None
        return {
            "entry_date": self.query_one("#f_date", Input).value.strip(),
            "start_time": self.query_one("#f_start", Input).value.strip() or "manual",
            "end_time": self.query_one("#f_end", Input).value.strip() or "manual",
            "hrs": hrs,
            "category": self.query_one("#f_cat", Select).value,
            "description": desc,
        }

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "ok":
            data = self._collect()
            if data is not None:
                self.dismiss(data)
        else:
            self.dismiss(None)

    def action_cancel(self) -> None:
        self.dismiss(None)


class LineItemForm(ModalScreen[dict | None]):
    """Edit an invoice line item (label / hours / narrative)."""

    BINDINGS = [Binding("escape", "cancel", "Cancel")]

    def __init__(self, item: invoice.LineItem) -> None:
        super().__init__()
        self._item = item

    def compose(self) -> ComposeResult:
        i = self._item
        with Vertical(id="form"):
            yield Label("Edit line item", id="modal-title")
            yield Label("Label")
            yield Input(value=i.label, id="li_label")
            yield Label("Hours")
            yield Input(value=f"{i.hours:.2f}", id="li_hours")
            yield Label("Narrative")
            yield Input(value=i.narrative, id="li_narr")
            with Horizontal(id="modal-buttons"):
                yield Button("Save", variant="success", id="ok")
                yield Button("Cancel [esc]", id="cancel")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "ok":
            try:
                hours = float(self.query_one("#li_hours", Input).value.strip())
            except ValueError:
                self.notify("Hours must be a number.", severity="error")
                return
            self.dismiss(
                {
                    "label": self.query_one("#li_label", Input).value.strip(),
                    "hours": hours,
                    "narrative": self.query_one("#li_narr", Input).value.strip(),
                }
            )
        else:
            self.dismiss(None)

    def action_cancel(self) -> None:
        self.dismiss(None)


class ConfirmStop(ModalScreen[bool]):
    """Confirm stopping a timer, showing the exact billing rule that will apply."""

    BINDINGS = [
        Binding("y,enter", "confirm", "Stop"),
        Binding("n,escape", "cancel", "Cancel"),
    ]

    def __init__(self, timer: store.RunningTimer, result: engine.StopResult) -> None:
        super().__init__()
        self._timer = timer
        self._result = result

    def compose(self) -> ComposeResult:
        r = self._result
        measured = engine.duration_hrs(r.start_time, r.end_time)
        notes = []
        if r.reboot_capped:
            notes.append("⚡ end capped at first boot after start (reboot spanned)")
        if r.hrs != measured:
            notes.append(f"↑ rounded up to next ¼h (measured {measured}h → billed {r.hrs}h)")
        else:
            notes.append(f"billed as measured — on a ¼h mark ({r.hrs}h)")
        with Vertical(id="modal"):
            yield Label(f"Stop timer #{self._timer.timer_id}?", id="modal-title")
            yield Static(f"[{r.category}] {r.description}")
            yield Static(f"{r.start_time}  →  {r.end_time}")
            yield Static("\n".join(f"• {n}" for n in notes), id="modal-notes")
            yield Static(f"Billable: {r.hrs}h  =  {_fmt_money(engine_billable(r))}")
            with Horizontal(id="modal-buttons"):
                yield Button("Stop [y]", variant="error", id="ok")
                yield Button("Cancel [n]", id="cancel")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "ok")

    def action_confirm(self) -> None:
        self.dismiss(True)

    def action_cancel(self) -> None:
        self.dismiss(False)


def engine_billable(result: engine.StopResult):
    from decimal import Decimal, ROUND_DOWN

    return (Decimal(result.hrs) * config.RATE).quantize(Decimal("0.01"), rounding=ROUND_DOWN)


_HELP_MD = """\
# stint — keys

**Global**
`Tab` switch pane · `r` refresh · `?` this help · `:` / `ctrl+p` command palette · `ctrl+t` theme · `q` quit

**Dashboard**
`s` focus quick-start · `enter` start timer · `x` stop selected timer

**Log**  (month browser + editor)
`◀ ▶` / `← →` change month · `a` add manual entry · `e` edit row · `d` delete row · type to search

**Invoice**
`← →` change month · pick client ▾ · `e` edit line · `d` drop line · `g` generate HTML

**Report**
`← →` change month — per-category and per-week rollups (read-only)

**Badges**  ✎ manual entry · +¼ rounded up to next quarter-hour · ⚡ reboot-capped (shown at stop time)
"""


class HelpScreen(ModalScreen[None]):
    """`?` overlay — keybindings + badge legend."""

    BINDINGS = [Binding("escape,q,question_mark", "dismiss", "Close")]

    def compose(self) -> ComposeResult:
        with Vertical(id="help"):
            yield Markdown(_HELP_MD)
            yield Static("[dim]esc to close[/dim]", id="help-foot")

    def action_dismiss(self) -> None:
        self.dismiss(None)


class StintCommands(Provider):
    """Command-palette actions beyond the auto-discovered bindings."""

    @property
    def _commands(self) -> list[tuple[str, str, "callable"]]:
        app = self.app
        cmds: list[tuple[str, str, callable]] = [
            ("Go to Dashboard", "Track: live timers + quick-start", lambda: app.show_pane("dash")),
            ("Go to Log", "Browse / edit month entries", lambda: app.show_pane("log")),
            ("Go to Invoice", "Consolidate + generate invoice", lambda: app.show_pane("invoice")),
            ("Go to Report", "Per-category / per-week rollups", lambda: app.show_pane("report")),
            ("Add manual entry", "Append a backdated/inferred entry", app.action_add_entry_anywhere),
            ("Jump to current month", "Reset Log + Invoice to this month", app.action_this_month),
            ("Generate invoice HTML", "Render the current invoice month", app.action_gen_invoice),
            ("Toggle theme", "Switch light / dark", app.action_toggle_theme),
            ("Help", "Show the keybinding overlay", app.action_help),
        ]
        return cmds

    async def discover(self) -> Hits:
        for title, help_text, run in self._commands:
            yield DiscoveryHit(title, run, help=help_text)

    async def search(self, query: str) -> Hits:
        matcher = self.matcher(query)
        for title, help_text, run in self._commands:
            score = matcher.match(title)
            if score > 0:
                yield Hit(score, matcher.highlight(title), run, help=help_text)


class StintApp(App):
    CSS_PATH = "app.tcss"
    TITLE = "stint"
    COMMANDS = App.COMMANDS | {StintCommands}

    BINDINGS = [
        Binding("q", "quit", "Quit"),
        Binding("r", "refresh", "Refresh"),
        Binding("question_mark", "help", "Help"),
        Binding("ctrl+t", "toggle_theme", "Theme"),
        Binding("x", "stop_timer", "Stop timer"),
        Binding("a", "add_entry", "Add"),
        Binding("e", "edit_entry", "Edit"),
        Binding("d", "delete_entry", "Delete"),
        Binding("g", "gen_invoice", "Generate"),
        Binding("left", "prev_month", "Prev month", show=False),
        Binding("right", "next_month", "Next month", show=False),
    ]

    def __init__(self) -> None:
        super().__init__()
        now = datetime.now()
        # In demo mode there's no "current month" data — land on the newest sample
        # month so the panes open populated instead of empty.
        months = store.available_months()
        if config.DEMO and months:
            year, month = months[0]
        else:
            year, month = now.year, now.month
        self.log_year = year
        self.log_month = month
        self._running_timers: dict[str, store.RunningTimer] = {}
        # Log pane: shown rows as (file_index, Entry) so edit/delete hit the right CSV row.
        self._log_shown: list[tuple[int, store.Entry]] = []
        # Invoice pane state.
        self.inv_year = year
        self.inv_month = month
        self.inv_client = config.DEFAULT_CLIENT
        self._inv_items: list[invoice.LineItem] = []
        self._inv_raw: float = 0.0
        # Report pane state.
        self.rep_year = year
        self.rep_month = month

    # ── Layout ──────────────────────────────────────────────────────────────

    def compose(self) -> ComposeResult:
        yield Static(id="totalsbar")
        with TabbedContent(initial="dash"):
            with TabPane("Dashboard", id="dash"):
                yield Label("RUNNING", classes="section")
                yield DataTable(id="running", cursor_type="row", zebra_stripes=True)
                with Horizontal(id="quickstart"):
                    yield Input(placeholder="What are you working on?", id="qs_desc")
                    yield Select(
                        [(c, c) for c in config.CATEGORIES],
                        value=config.DEFAULT_CATEGORY,
                        allow_blank=False,
                        id="qs_cat",
                    )
                    yield Button("Start ▸", variant="success", id="qs_start")
                yield Label("RECENT (today)", classes="section")
                yield DataTable(id="recent", cursor_type="row", zebra_stripes=True)
            with TabPane("Log", id="log"):
                with Horizontal(id="logbar"):
                    yield Button("◀", id="prev_month")
                    yield Static(id="month_label")
                    yield Button("▶", id="next_month")
                    yield Select(
                        [("all categories", _ALL)] + [(c, c) for c in config.CATEGORIES],
                        value=_ALL,
                        allow_blank=False,
                        id="filter_cat",
                    )
                    yield Input(placeholder="search…", id="search")
                yield DataTable(id="logtable", cursor_type="row", zebra_stripes=True)
                with Horizontal(id="logactions"):
                    yield Button("+ Add [a]", id="log_add", variant="success")
                    yield Button("Edit [e]", id="log_edit")
                    yield Button("Delete [d]", id="log_delete", variant="error")
                yield Static(id="log_total")
            with TabPane("Invoice", id="invoice"):
                with Horizontal(id="invbar"):
                    yield Button("◀", id="inv_prev")
                    yield Static(id="inv_month_label")
                    yield Button("▶", id="inv_next")
                    yield Select(
                        [(c.display_name, k) for k, c in config.CLIENTS.items()],
                        value=config.DEFAULT_CLIENT,
                        allow_blank=False,
                        id="inv_client",
                    )
                    yield Static(id="inv_next_num")
                yield DataTable(id="invtable", cursor_type="row", zebra_stripes=True)
                yield Static(id="inv_delta")
                with Horizontal(id="invactions"):
                    yield Button("Edit line [e]", id="inv_edit")
                    yield Button("Drop line [d]", id="inv_drop", variant="error")
                    yield Button("Generate HTML [g]", id="inv_gen", variant="success")
            with TabPane("Report", id="report"):
                with Horizontal(id="repbar"):
                    yield Button("◀", id="rep_prev")
                    yield Static(id="rep_month_label")
                    yield Button("▶", id="rep_next")
                    yield Static(id="rep_summary")
                yield Label("BY CATEGORY", classes="section")
                yield DataTable(id="rep_cat", cursor_type="row", zebra_stripes=True)
                yield Label("BY WEEK (Mon–Sun)", classes="section")
                yield DataTable(id="rep_week", cursor_type="row", zebra_stripes=True)
        yield Footer()

    def on_mount(self) -> None:
        self.register_theme(STINT_THEME)
        self.theme = "stint"
        rt = self.query_one("#running", DataTable)
        rt.add_columns("#", "Cat", "Description", "Elapsed", "$ so far")
        rc = self.query_one("#recent", DataTable)
        rc.add_columns("Time", "Cat", "Description", "Hrs", "$")
        lt = self.query_one("#logtable", DataTable)
        lt.add_columns("Date", "Start", "End", "Hrs", "Cat", "Description", "$")
        it = self.query_one("#invtable", DataTable)
        it.add_columns("Label", "Hrs", "Total", "Narrative")
        rcat = self.query_one("#rep_cat", DataTable)
        rcat.add_columns("Category", "Entries", "Hours", "Amount", "Share")
        rwk = self.query_one("#rep_week", DataTable)
        rwk.add_columns("Week", "Days", "Entries", "Hours", "Amount")
        self.refresh_all()
        self.refresh_invoice()
        self.refresh_report()
        self.set_interval(1.0, self.tick)

    # ── Live tick ─────────────────────────────────────────────────────────────

    def tick(self) -> None:
        now = datetime.now()
        self.update_totalsbar(now)
        # If the set of running timers changed on disk, rebuild; else update cells.
        on_disk = {str(t.timer_id): t for t in store.read_timers()}
        if set(on_disk) != set(self._running_timers):
            self.refresh_running()
        else:
            table = self.query_one("#running", DataTable)
            for key, timer in on_disk.items():
                try:
                    table.update_cell(key, "elapsed", timer.elapsed_hms(now))
                    table.update_cell(key, "money", _fmt_money(timer.live_amount(now)))
                except Exception:
                    self.refresh_running()
                    break

    # ── Refresh ───────────────────────────────────────────────────────────────

    def refresh_all(self) -> None:
        self.update_totalsbar(datetime.now())
        self.refresh_running()
        self.refresh_recent()
        self.refresh_log()

    def update_totalsbar(self, now: datetime) -> None:
        s = store.summary(now.date())
        t, w, m = s["today"], s["week"], s["month"]
        clock = now.strftime("%H:%M:%S")
        demo = "[reverse] DEMO [/reverse] " if config.DEMO else ""
        text = (
            f"{demo}[b]stint[/b]   {clock}    "
            f"Today [b]{t.hours:.2f}h[/b] {_fmt_money(t.amount)}  │  "
            f"Wk [b]{w.hours:.2f}h[/b] {_fmt_money(w.amount)}  │  "
            f"{now.strftime('%b')} [b]{m.hours:.2f}h[/b] {_fmt_money(m.amount)}"
        )
        self.query_one("#totalsbar", Static).update(text)

    def refresh_running(self) -> None:
        table = self.query_one("#running", DataTable)
        # Ensure stable column keys for in-place cell updates.
        if "elapsed" not in table.columns:
            table.clear(columns=True)
            table.add_column("#", key="id")
            table.add_column("Cat", key="cat")
            table.add_column("Description", key="desc")
            table.add_column("Elapsed", key="elapsed")
            table.add_column("$ so far", key="money")
        else:
            table.clear()
        now = datetime.now()
        self._running_timers = {}
        for t in store.read_timers():
            key = str(t.timer_id)
            self._running_timers[key] = t
            table.add_row(
                str(t.timer_id), t.category, t.description,
                t.elapsed_hms(now), _fmt_money(t.live_amount(now)),
                key=key,
            )
        if not self._running_timers:
            table.add_row("—", "", "no timers running — start one below", "", "")

    def refresh_recent(self) -> None:
        table = self.query_one("#recent", DataTable)
        table.clear()
        today = datetime.now().date()
        rows = [e for e in store.read_month(today.year, today.month) if e.date == today]
        for e in reversed(rows):
            start = e.start_time if not e.is_manual else "manual"
            desc = e.description + _entry_badges(e)
            table.add_row(start, e.category, desc, f"{e.hours:.2f}", _fmt_money(e.amount))
        if not rows:
            table.add_row("—", "", "nothing logged today yet", "", "")

    def refresh_log(self) -> None:
        table = self.query_one("#logtable", DataTable)
        table.clear()
        cat = self.query_one("#filter_cat", Select).value
        needle = self.query_one("#search", Input).value.strip().lower()
        entries = store.read_month(self.log_year, self.log_month)
        # Keep the file index so edit/delete target the right CSV row after filtering.
        self._log_shown = []
        for idx, e in enumerate(entries):
            if cat != _ALL and e.category != cat:
                continue
            if needle and needle not in e.description.lower():
                continue
            self._log_shown.append((idx, e))
        for idx, e in self._log_shown:
            table.add_row(
                e.entry_date,
                e.start_time,
                e.end_time,
                f"{e.hours:.2f}",
                e.category,
                e.description + _entry_badges(e),
                _fmt_money(e.amount),
                key=str(idx),
            )
        total_h = sum((e.hours for _, e in self._log_shown), Decimal(0))
        total_a = sum((e.amount for _, e in self._log_shown), Decimal(0))
        label = f"{self.log_year:04d}-{self.log_month:02d}"
        self.query_one("#month_label", Static).update(f"  {label}  ")
        n = len(self._log_shown)
        self.query_one("#log_total", Static).update(
            f"  {n} " + ("entry" if n == 1 else "entries") +
            f"   ·   {total_h:.2f}h   {_fmt_money(total_a)}"
        )

    def _selected_log_index(self) -> int | None:
        """File index of the log row under the cursor, or None."""
        table = self.query_one("#logtable", DataTable)
        try:
            key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
            return int(key)
        except Exception:
            return None

    # ── Invoice pane ──────────────────────────────────────────────────────────

    def refresh_invoice(self) -> None:
        table = self.query_one("#invtable", DataTable)
        table.clear()
        y, m = self.inv_year, self.inv_month
        # Prefer a staged (hand/inline-edited) set if present, else compute fresh.
        staged = invoice.read_staging(y, m)
        if staged is not None:
            self._inv_items = staged
            try:
                _items, self._inv_raw = invoice.line_items_for(y, m)
            except FileNotFoundError:
                self._inv_raw = sum(i.hours for i in staged)
            source = "staged"
        else:
            try:
                self._inv_items, self._inv_raw = invoice.line_items_for(y, m)
            except FileNotFoundError:
                self._inv_items, self._inv_raw = [], 0.0
            source = "computed"
        for i, item in enumerate(self._inv_items):
            table.add_row(item.label, f"{item.hours:.2f}", _fmt_money(item.total), item.narrative, key=str(i))

        inv_total_h = sum(i.hours for i in self._inv_items)
        inv_total_a = sum(i.total for i in self._inv_items)
        raw_a = self._inv_raw * invoice.RATE
        delta = inv_total_a - raw_a
        delta_str = f"+{_fmt_money(delta)}" if delta >= 0 else f"-{_fmt_money(abs(delta))}"
        self.query_one("#inv_month_label", Static).update(f"  {y:04d}-{m:02d}  ")
        self.query_one("#inv_delta", Static).update(
            f"  Raw CSV: {self._inv_raw:.2f}h ({_fmt_money(raw_a)})    "
            f"Invoice: {inv_total_h:.2f}h ({_fmt_money(inv_total_a)})    "
            f"Δ {delta_str} ({source})"
        )
        client = config.CLIENTS[self.inv_client]
        try:
            nxt = invoice.next_invoice_number(self.inv_client)
        except Exception:
            nxt = client.counter_seed
        cap = "" if client.html_capable else "  ⚠ hand-built (PHP/EWT) — number only"
        self.query_one("#inv_next_num", Static).update(
            f"  Next #: {nxt} · {client.display_name} ({client.currency}){cap}"
        )

    def _selected_inv_index(self) -> int | None:
        table = self.query_one("#invtable", DataTable)
        try:
            key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
            return int(key)
        except Exception:
            return None

    def _shift_inv_month(self, delta: int) -> None:
        m = self.inv_month + delta
        y = self.inv_year
        if m < 1:
            m, y = 12, y - 1
        elif m > 12:
            m, y = 1, y + 1
        self.inv_year, self.inv_month = y, m
        self.refresh_invoice()

    def _persist_staging(self) -> None:
        invoice.write_staging(self._inv_items, self.inv_year, self.inv_month, today=datetime.now().date())

    def action_gen_invoice(self) -> None:
        # Only meaningful on the Invoice tab.
        if self.query_one(TabbedContent).active != "invoice":
            return
        client = config.CLIENTS[self.inv_client]
        if not client.html_capable:
            self.notify(
                f"{client.display_name} invoices are hand-built ({client.currency}/EWT). "
                f"Number {invoice.next_invoice_number(self.inv_client)} is reserved; build the HTML manually.",
                severity="warning",
                timeout=8,
            )
            return
        if not self._inv_items:
            self.notify("No line items to invoice.", severity="warning")
            return
        y, m = self.inv_year, self.inv_month
        out = invoice.out_path_for(y, m)
        msg = f"Generate {out.name} for {client.display_name}?"
        if out.exists():
            msg += "\n(overwrites the existing file)"

        def _done(ok: bool | None) -> None:
            if not ok:
                return
            try:
                entries = invoice.load_entries(y, m)
            except FileNotFoundError:
                entries = []
            inv_num = invoice.next_invoice_number(self.inv_client)
            html = invoice.build_html(self._inv_items, entries, y, m, inv_num=inv_num, client_key=self.inv_client)
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(html, encoding="utf-8")
            invoice.commit_invoice_number(inv_num, self.inv_client)
            self.notify(f"Invoice #{inv_num} → {out}", timeout=8)
            self.refresh_invoice()

        self.push_screen(Confirm(msg, yes_label="Generate", yes_variant="success"), _done)

    # ── Report pane ───────────────────────────────────────────────────────────

    def refresh_report(self) -> None:
        y, m = self.rep_year, self.rep_month
        entries = store.read_month(y, m)

        # By category (ordered by config so the table is stable), share by hours.
        cat_tbl = self.query_one("#rep_cat", DataTable)
        cat_tbl.clear()
        total_h = sum((e.hours for e in entries), Decimal(0))
        total_a = sum((e.amount for e in entries), Decimal(0))
        by_cat: "OrderedDict[str, list]" = OrderedDict((c, []) for c in config.CATEGORIES)
        for e in entries:
            by_cat.setdefault(e.category, []).append(e)
        for cat, rows in by_cat.items():
            if not rows:
                continue
            h = sum((e.hours for e in rows), Decimal(0))
            a = sum((e.amount for e in rows), Decimal(0))
            share = (h / total_h * 100) if total_h else Decimal(0)
            cat_tbl.add_row(cat, str(len(rows)), f"{h:.2f}", _fmt_money(a), f"{share:.0f}%")
        if not entries:
            cat_tbl.add_row("—", "0", "0.00", _fmt_money(0), "—")

        # By ISO week (Mon–Sun), keyed on the Monday date.
        week_tbl = self.query_one("#rep_week", DataTable)
        week_tbl.clear()
        by_week: "OrderedDict[str, list]" = OrderedDict()
        for e in sorted(entries, key=lambda e: e.date):
            monday = e.date - timedelta(days=e.date.weekday())
            by_week.setdefault(monday.isoformat(), []).append(e)
        for monday_iso, rows in by_week.items():
            monday = datetime.strptime(monday_iso, "%Y-%m-%d").date()
            sunday = monday + timedelta(days=6)
            h = sum((e.hours for e in rows), Decimal(0))
            a = sum((e.amount for e in rows), Decimal(0))
            span = f"{monday.strftime('%b %-d')}–{sunday.strftime('%-d')}"
            week_tbl.add_row(span, "", str(len(rows)), f"{h:.2f}", _fmt_money(a))
        if not entries:
            week_tbl.add_row("—", "", "0", "0.00", _fmt_money(0))

        self.query_one("#rep_month_label", Static).update(f"  {y:04d}-{m:02d}  ")
        self.query_one("#rep_summary", Static).update(
            f"  {len(entries)} entries · {total_h:.2f}h · {_fmt_money(total_a)}"
        )

    def _shift_rep_month(self, delta: int) -> None:
        m = self.rep_month + delta
        y = self.rep_year
        if m < 1:
            m, y = 12, y - 1
        elif m > 12:
            m, y = 1, y + 1
        self.rep_year, self.rep_month = y, m
        self.refresh_report()

    # ── Actions ─────────────────────────────────────────────────────────────

    def action_refresh(self) -> None:
        self.refresh_all()
        self.refresh_invoice()
        self.refresh_report()

    def action_help(self) -> None:
        self.push_screen(HelpScreen())

    def action_toggle_theme(self) -> None:
        self.theme = "textual-light" if self.theme != "textual-light" else "stint"

    def show_pane(self, pane: str) -> None:
        self.query_one(TabbedContent).active = pane

    def action_this_month(self) -> None:
        now = datetime.now()
        self.log_year, self.log_month = now.year, now.month
        self.inv_year, self.inv_month = now.year, now.month
        self.rep_year, self.rep_month = now.year, now.month
        self.refresh_log()
        self.refresh_invoice()
        self.refresh_report()

    def action_add_entry_anywhere(self) -> None:
        """Palette 'Add manual entry' — works regardless of active pane."""
        def _done(data: dict | None) -> None:
            if not data:
                return
            store.add_entry(
                description=data["description"], category=data["category"],
                hrs=data["hrs"], entry_date=data["entry_date"],
            )
            self.notify(f"Added: [{data['category']}] {data['description']}")
            self.refresh_all()
            self.refresh_report()

        self.push_screen(EntryForm(), _done)

    def _active_tab(self) -> str:
        return self.query_one(TabbedContent).active

    def action_prev_month(self) -> None:
        tab = self._active_tab()
        if tab == "invoice":
            self._shift_inv_month(-1)
        elif tab == "report":
            self._shift_rep_month(-1)
        else:
            self._shift_month(-1)

    def action_next_month(self) -> None:
        tab = self._active_tab()
        if tab == "invoice":
            self._shift_inv_month(1)
        elif tab == "report":
            self._shift_rep_month(1)
        else:
            self._shift_month(1)

    def _shift_month(self, delta: int) -> None:
        m = self.log_month + delta
        y = self.log_year
        if m < 1:
            m, y = 12, y - 1
        elif m > 12:
            m, y = 1, y + 1
        self.log_year, self.log_month = y, m
        self.refresh_log()

    def action_stop_timer(self) -> None:
        table = self.query_one("#running", DataTable)
        if not self._running_timers:
            self.notify("No timers running.", severity="warning")
            return
        try:
            row_key = table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value
        except Exception:
            row_key = None
        timer = self._running_timers.get(row_key) if row_key else None
        if timer is None:
            # Default to the single running timer if the cursor isn't on one.
            if len(self._running_timers) == 1:
                timer = next(iter(self._running_timers.values()))
            else:
                self.notify("Select a timer row, then press x.", severity="warning")
                return
        # Dry-run the engine to preview the rule; reuse that end_time on confirm.
        end_time = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        preview = engine.stop(
            entry_date=timer.entry_date, start_time=timer.start,
            category=timer.category, description=timer.description, end_time=end_time,
        )

        def _done(confirmed: bool | None) -> None:
            if confirmed:
                result = store.stop_timer(timer, end_time=end_time)
                self.notify(f"Stopped #{timer.timer_id} — {result.hrs}h logged.")
                self.refresh_all()

        self.push_screen(ConfirmStop(timer, preview), _done)

    # ── Entry edit actions (Log pane) ─────────────────────────────────────────

    def action_add_entry(self) -> None:
        if self._active_tab() != "log":
            return

        def _done(data: dict | None) -> None:
            if not data:
                return
            store.add_entry(
                description=data["description"], category=data["category"],
                hrs=data["hrs"], entry_date=data["entry_date"],
            )
            self.notify(f"Added: [{data['category']}] {data['description']}")
            self.refresh_all()

        self.push_screen(EntryForm(), _done)

    def action_edit_entry(self) -> None:
        tab = self._active_tab()
        if tab == "invoice":
            self._edit_inv_line()
            return
        if tab != "log":
            return
        idx = self._selected_log_index()
        if idx is None:
            self.notify("Select a log row to edit.", severity="warning")
            return
        entries = store.read_month(self.log_year, self.log_month)
        if not (0 <= idx < len(entries)):
            self.refresh_log()
            return
        y, m = self.log_year, self.log_month

        def _done(data: dict | None) -> None:
            if not data:
                return
            store.update_entry(y, m, idx, **data)
            self.notify("Entry updated.")
            self.refresh_all()

        self.push_screen(EntryForm(entries[idx]), _done)

    def action_delete_entry(self) -> None:
        tab = self._active_tab()
        if tab == "invoice":
            self._drop_inv_line()
            return
        if tab != "log":
            return
        idx = self._selected_log_index()
        if idx is None:
            self.notify("Select a log row to delete.", severity="warning")
            return
        entries = store.read_month(self.log_year, self.log_month)
        if not (0 <= idx < len(entries)):
            self.refresh_log()
            return
        y, m = self.log_year, self.log_month
        target = entries[idx]

        def _done(ok: bool | None) -> None:
            if ok:
                store.delete_entry(y, m, idx)
                self.notify("Entry deleted.")
                self.refresh_all()

        self.push_screen(
            Confirm(f"Delete this entry?\n[{target.category}] {target.description}"), _done
        )

    # ── Invoice line-item actions ─────────────────────────────────────────────

    def _edit_inv_line(self) -> None:
        idx = self._selected_inv_index()
        if idx is None or not (0 <= idx < len(self._inv_items)):
            self.notify("Select a line item to edit.", severity="warning")
            return

        def _done(data: dict | None) -> None:
            if not data:
                return
            item = self._inv_items[idx]
            item.label = data["label"]
            item.hours = data["hours"]
            item.narrative = data["narrative"]
            self._persist_staging()
            self.notify("Line item updated (staged).")
            self.refresh_invoice()

        self.push_screen(LineItemForm(self._inv_items[idx]), _done)

    def _drop_inv_line(self) -> None:
        idx = self._selected_inv_index()
        if idx is None or not (0 <= idx < len(self._inv_items)):
            self.notify("Select a line item to drop.", severity="warning")
            return
        item = self._inv_items[idx]

        def _done(ok: bool | None) -> None:
            if ok:
                del self._inv_items[idx]
                self._persist_staging()
                self.notify("Line item dropped (staged).")
                self.refresh_invoice()

        self.push_screen(Confirm(f"Drop line item?\n{item.label} ({item.hours:.2f}h)"), _done)

    # ── Widget events ─────────────────────────────────────────────────────────

    def on_button_pressed(self, event: Button.Pressed) -> None:
        bid = event.button.id
        if bid == "qs_start":
            self._quick_start()
        elif bid == "prev_month":
            self._shift_month(-1)
        elif bid == "next_month":
            self._shift_month(1)
        elif bid == "log_add":
            self.action_add_entry()
        elif bid == "log_edit":
            self.action_edit_entry()
        elif bid == "log_delete":
            self.action_delete_entry()
        elif bid == "inv_prev":
            self._shift_inv_month(-1)
        elif bid == "inv_next":
            self._shift_inv_month(1)
        elif bid == "inv_edit":
            self._edit_inv_line()
        elif bid == "inv_drop":
            self._drop_inv_line()
        elif bid == "inv_gen":
            self.action_gen_invoice()
        elif bid == "rep_prev":
            self._shift_rep_month(-1)
        elif bid == "rep_next":
            self._shift_rep_month(1)

    def _quick_start(self) -> None:
        desc_input = self.query_one("#qs_desc", Input)
        desc = desc_input.value.strip()
        if not desc:
            self.notify("Enter a description first.", severity="warning")
            return
        cat = self.query_one("#qs_cat", Select).value
        timer = store.start_timer(desc, cat)
        desc_input.value = ""
        self.notify(f"Started #{timer.timer_id} — [{cat}] {desc}")
        self.refresh_running()
        self.update_totalsbar(datetime.now())

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id == "qs_desc":
            self._quick_start()
        elif event.input.id == "search":
            self.refresh_log()

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "search":
            self.refresh_log()

    def on_select_changed(self, event: Select.Changed) -> None:
        if event.select.id == "filter_cat":
            self.refresh_log()
        elif event.select.id == "inv_client":
            self.inv_client = event.value
            self.refresh_invoice()
