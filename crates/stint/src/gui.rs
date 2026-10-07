//! `stint gui`: the egui/eframe desktop app.
//!
//! Four panes mirroring the Textual TUI (Dashboard / Log / Invoice / Report)
//! over the same `stint-core` calls. The ledger is re-read once a second, so
//! anything the CLI, the Python TUI or a Claude Code session writes shows up
//! live without a daemon. Writes go through `store` / `invoice` only.

use std::time::{Duration, Instant};

use chrono::{Datelike, Local, NaiveDate};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use stint_core::{config, engine, invoice, report, store, Home};

const TICK: Duration = Duration::from_secs(1);
const ACCENT: Color32 = Color32::from_rgb(0x4a, 0x9e, 0xff);
const WARN: Color32 = Color32::from_rgb(0xd2, 0x99, 0x22);
const OK: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
const ERR: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Dashboard,
    Log,
    Invoice,
    Report,
}

impl Pane {
    const ALL: [Pane; 4] = [Pane::Dashboard, Pane::Log, Pane::Invoice, Pane::Report];
    fn parse(s: &str) -> Option<Pane> {
        Pane::ALL.iter().copied().find(|p| p.title().eq_ignore_ascii_case(s.trim()))
    }
    fn title(self) -> &'static str {
        match self {
            Pane::Dashboard => "Dashboard",
            Pane::Log => "Log",
            Pane::Invoice => "Invoice",
            Pane::Report => "Report",
        }
    }
}

/// Editable copy of an entry (all strings, validated on save).
#[derive(Clone, Default)]
struct EntryForm {
    index: Option<usize>, // None = add
    date: String,
    start: String,
    end: String,
    hrs: String,
    category: String,
    description: String,
    error: String,
}

impl EntryForm {
    fn from_entry(index: usize, e: &store::Entry) -> Self {
        EntryForm {
            index: Some(index),
            date: e.entry_date.clone(),
            start: e.start_time.clone(),
            end: e.end_time.clone(),
            hrs: e.hrs.clone(),
            category: e.category.clone(),
            description: e.description.clone(),
            error: String::new(),
        }
    }

    fn blank(today: NaiveDate) -> Self {
        EntryForm {
            index: None,
            date: today.format("%Y-%m-%d").to_string(),
            start: store::MANUAL.into(),
            end: store::MANUAL.into(),
            hrs: "0.25".into(),
            category: config::DEFAULT_CATEGORY.into(),
            description: String::new(),
            error: String::new(),
        }
    }

    fn to_entry(&self) -> Result<store::Entry, String> {
        let date = NaiveDate::parse_from_str(self.date.trim(), "%Y-%m-%d").map_err(|_| "date must be YYYY-MM-DD".to_string())?;
        let hrs: Decimal = self.hrs.trim().parse().map_err(|_| "hours must be a decimal like 1.5".to_string())?;
        if hrs.is_sign_negative() {
            return Err("hours must not be negative".into());
        }
        let start = self.start.trim();
        let end = self.end.trim();
        let manual = start.is_empty() || start == store::MANUAL;
        let (start, end) = if manual {
            (store::MANUAL.to_string(), store::MANUAL.to_string())
        } else {
            for t in [start, end] {
                chrono::NaiveTime::parse_from_str(t, "%H:%M:%S").map_err(|_| format!("time {t:?} must be HH:MM:SS (or `manual`)"))?;
            }
            (start.to_string(), end.to_string())
        };
        if self.description.trim().is_empty() {
            return Err("description is required".into());
        }
        Ok(store::Entry {
            entry_date: date.format("%Y-%m-%d").to_string(),
            start_time: start,
            end_time: end,
            hrs: self.hrs.trim().to_string(),
            category: self.category.trim().to_string(),
            description: self.description.trim().to_string(),
        })
    }
}

#[derive(Clone, Default)]
struct LineForm {
    index: usize,
    label: String,
    hours: String,
    narrative: String,
    error: String,
}

struct App {
    home: Home,
    pane: Pane,
    dark: bool,
    last_tick: Instant,
    status: (String, Color32),
    /// Fingerprint of the ledger's mtimes at the last refresh; a tick that
    /// finds it unchanged skips re-reading (matters over \\wsl.localhost).
    ledger_stamp: u128,
    /// Ledger-folder picker: shown on first run (no ledger found) or on demand.
    home_form: Option<(String, String)>, // (path text, error)
    /// Open a generated invoice in the browser (off in headless tests).
    open_generated: bool,
    /// Cached per refresh: months on disk, this month's entries (dashboard),
    /// and the next invoice number. Reading these per frame hammered the
    /// ledger over the WSL share and, for the counter, seeded the file.
    months: Vec<(i32, u32)>,
    recent: Vec<store::Entry>,
    inv_next: Option<u32>,

    // dashboard
    timers: Vec<store::RunningTimer>,
    summary: Option<store::Summary>,
    quick_desc: String,
    quick_cat: String,
    stop_confirm: Option<(store::RunningTimer, engine::StopResult)>,

    // log
    log_ym: (i32, u32),
    log_entries: Vec<store::Entry>,
    log_cat: String, // "" = all
    log_search: String,
    log_selected: Option<usize>,
    entry_form: Option<EntryForm>,
    delete_confirm: Option<usize>,

    // invoice
    inv_ym: (i32, u32),
    inv_client: String,
    inv_items: Vec<invoice::LineItem>,
    inv_raw_total: f64,
    inv_staged: bool,
    inv_error: String,
    line_form: Option<LineForm>,
    inv_selected: Option<usize>,

    // report
    rep_ym: (i32, u32),
    rep_entries: Vec<store::Entry>,
    /// Every entry of the report year (for the calendar heatmap).
    rep_year_entries: Vec<store::Entry>,
}

fn this_month() -> (i32, u32) {
    let t = Local::now().date_naive();
    (t.year(), t.month())
}

fn shift_month((y, m): (i32, u32), delta: i32) -> (i32, u32) {
    let idx = y * 12 + (m as i32 - 1) + delta;
    (idx.div_euclid(12), (idx.rem_euclid(12) + 1) as u32)
}

/// GUI rendering of `report::badges`: egui's default fonts lack the pencil
/// glyph the TUI uses for manual entries, so it becomes a short word, with
/// the meaning on hover.
fn badge_label(ui: &mut egui::Ui, e: &store::Entry) {
    let badges = report::badges(e);
    if badges.is_empty() {
        return;
    }
    let text: Vec<&str> = badges.iter().map(|b| if *b == "✎" { "manual" } else { b }).collect();
    let tip: Vec<&str> = badges
        .iter()
        .map(|b| match *b {
            "✎" => "manual entry: hours typed, not measured",
            "+¼" => "rounded up to the next quarter hour",
            _ => "",
        })
        .collect();
    ui.label(RichText::new(text.join(" ")).color(WARN).small()).on_hover_text(tip.join("\n"));
}

fn money(d: Decimal) -> String {
    format!("${d:.2}")
}

/// Open a generated file in the default browser. Inside WSL a Linux path
/// would be handed to the Windows browser as `file://wsl$/…`, which Chrome
/// refuses, so translate it with `wslpath -w` and hand the UNC path to
/// explorer.exe instead.
fn open_in_browser(path: &std::path::Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let in_wsl = std::fs::read_to_string("/proc/version").is_ok_and(|v| v.to_lowercase().contains("microsoft"));
        if in_wsl {
            let win = std::process::Command::new("wslpath").arg("-w").arg(path).output();
            if let Ok(out) = win {
                let winpath = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !winpath.is_empty() {
                    return std::process::Command::new("explorer.exe").arg(&winpath).spawn().is_ok();
                }
            }
        }
    }
    open::that(path).is_ok()
}

/// Right-aligned numeric cell, monospace, so columns of figures line up on
/// the decimal point instead of reading like a left-justified text dump.
fn num_cell(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(RichText::new(text.into()).monospace());
    });
}

fn head_cell(ui: &mut egui::Ui, text: &str, right: bool) {
    if right {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(text).strong());
        });
    } else {
        ui.label(RichText::new(text).strong());
    }
}

/// Heat colour for `v` in 0..=1: five steps from the empty-cell grey to the
/// accent, like GitHub's contribution graph.
fn heat_color(v: f32, dark: bool) -> Color32 {
    let empty = if dark { Color32::from_gray(38) } else { Color32::from_gray(235) };
    if v <= 0.0 {
        return empty;
    }
    let step = ((v * 4.0).ceil() as i32).clamp(1, 4);
    let t = step as f32 / 4.0;
    let (r, g, b) = (0x4a as f32, 0x9e as f32, 0xff as f32);
    let base = if dark { 30.0 } else { 215.0 };
    let mix = |c: f32| (base + (c - base) * (0.35 + 0.65 * t)) as u8;
    Color32::from_rgb(mix(r), mix(g), mix(b))
}

fn fmt_h(d: Decimal) -> String {
    format!("{d:.2}h")
}

/// One row of equally sized heat cells with labels underneath, e.g. 24 hours
/// or 7 weekdays. `label_every` thins the axis labels.
fn heat_strip(ui: &mut egui::Ui, values: &[Decimal], labels: &[String], label_every: usize, dark: bool, tip: impl Fn(usize) -> String) {
    let n = values.len();
    let max = values.iter().copied().fold(Decimal::ZERO, Decimal::max);
    let avail = ui.available_width();
    let gap = 3.0;
    let cell = ((avail - gap * (n as f32 - 1.0)) / n as f32).clamp(8.0, 36.0);
    let h = 18.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(n as f32 * (cell + gap), h + 16.0), egui::Sense::hover());
    let painter = ui.painter();
    for (i, v) in values.iter().enumerate() {
        let x = rect.left() + i as f32 * (cell + gap);
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(cell, h));
        let frac = if max.is_zero() { 0.0 } else { (*v / max).to_f32().unwrap_or(0.0) };
        painter.rect_filled(r, 3.0, heat_color(frac, dark));
        if i % label_every == 0 {
            painter.text(egui::pos2(x, rect.top() + h + 2.0), egui::Align2::LEFT_TOP, &labels[i], egui::FontId::proportional(10.0), ui.visuals().weak_text_color());
        }
        if ui.rect_contains_pointer(r) {
            egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::new(("heat_strip", i)), egui::PopupAnchor::Pointer).show(|ui| {
                ui.label(tip(i));
            });
        }
    }
}

/// GitHub-style year calendar: one column per ISO week, one row per weekday
/// (Mon top), coloured by hours logged that day.
fn heat_calendar(ui: &mut egui::Ui, year: i32, daily: &std::collections::BTreeMap<NaiveDate, store::Totals>, dark: bool) {
    let jan1 = NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
    let dec31 = NaiveDate::from_ymd_opt(year, 12, 31).unwrap();
    // Column 0 starts on the Monday on/before Jan 1.
    let origin = jan1 - chrono::Duration::days(jan1.weekday().num_days_from_monday() as i64);
    let weeks = ((dec31 - origin).num_days() / 7 + 1) as usize;
    let max = daily.values().map(|t| t.hours).fold(Decimal::ZERO, Decimal::max);
    let gap = 2.0;
    let cell = ((ui.available_width() - 30.0 - gap * weeks as f32) / weeks as f32).clamp(6.0, 14.0);
    let left = 30.0; // weekday labels
    let top = 14.0; // month labels
    let size = egui::vec2(left + weeks as f32 * (cell + gap), top + 7.0 * (cell + gap));
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let weak = ui.visuals().weak_text_color();
    for (row, name) in ["Mon", "", "Wed", "", "Fri", "", "Sun"].iter().enumerate() {
        if !name.is_empty() {
            painter.text(egui::pos2(rect.left(), rect.top() + top + row as f32 * (cell + gap) + cell / 2.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(10.0), weak);
        }
    }
    let mut last_month = 0;
    let today = Local::now().date_naive();
    for w in 0..weeks {
        for d in 0..7 {
            let date = origin + chrono::Duration::days((w * 7 + d) as i64);
            if date.year() != year {
                continue;
            }
            let x = rect.left() + left + w as f32 * (cell + gap);
            let y = rect.top() + top + d as f32 * (cell + gap);
            if date.month() != last_month && d == 0 {
                last_month = date.month();
                painter.text(egui::pos2(x, rect.top()), egui::Align2::LEFT_TOP, date.format("%b").to_string(), egui::FontId::proportional(10.0), weak);
            }
            let day = daily.get(&date).copied().unwrap_or_default();
            let hrs = day.hours;
            let frac = if max.is_zero() { 0.0 } else { (hrs / max).to_f32().unwrap_or(0.0) };
            let r = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell, cell));
            let mut color = heat_color(frac, dark);
            if date > today {
                color = color.gamma_multiply(0.35);
            }
            painter.rect_filled(r, 2.0, color);
            if ui.rect_contains_pointer(r) {
                egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::new(("heat_cal", w, d)), egui::PopupAnchor::Pointer).show(|ui| {
                    ui.label(format!("{}  {}  {}", date.format("%a %-d %b %Y"), fmt_h(hrs), money(day.amount)));
                });
            }
        }
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, home: Home) -> Self {
        Self::with_ctx(&cc.egui_ctx, home)
    }

    /// Build the app for an existing egui context (headless tests use a bare
    /// `egui::Context`; eframe passes its own).
    fn with_ctx(ctx: &egui::Context, home: Home) -> Self {
        let dark = ctx.theme() == egui::Theme::Dark;
        let start_ym = if home.demo {
            store::available_months(&home).first().copied().unwrap_or_else(this_month)
        } else {
            this_month()
        };
        let default_client = home.default_client.clone();
        let mut app = App {
            home,
            pane: Pane::Dashboard,
            dark,
            last_tick: Instant::now() - TICK,
            status: (String::new(), ACCENT),
            ledger_stamp: 0,
            home_form: None,
            open_generated: true,
            months: Vec::new(),
            recent: Vec::new(),
            inv_next: None,
            timers: Vec::new(),
            summary: None,
            quick_desc: String::new(),
            quick_cat: config::DEFAULT_CATEGORY.into(),
            stop_confirm: None,
            log_ym: start_ym,
            log_entries: Vec::new(),
            log_cat: String::new(),
            log_search: String::new(),
            log_selected: None,
            entry_form: None,
            delete_confirm: None,
            inv_ym: start_ym,
            inv_client: default_client,
            inv_items: Vec::new(),
            inv_raw_total: 0.0,
            inv_staged: false,
            inv_error: String::new(),
            line_form: None,
            inv_selected: None,
            rep_ym: start_ym,
            rep_entries: Vec::new(),
            rep_year_entries: Vec::new(),
        };
        app.refresh_all();
        if !app.home.looks_like_ledger() {
            app.home_form = Some((app.home.log_dir.display().to_string(), String::new()));
        }
        app
    }

    /// Max mtime over the ledger's top-level files, `.timers/` and `temp/`,
    /// folded with sizes into one number. Three directory listings per second
    /// are cheap even over 9P; re-parsing every CSV is not.
    fn ledger_stamp(&self) -> u128 {
        fn fold(dir: &std::path::Path, acc: &mut u128) {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    if let Ok(md) = e.metadata() {
                        let ns = md
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_nanos())
                            .unwrap_or(0);
                        *acc = acc.wrapping_mul(31).wrapping_add(ns ^ (md.len() as u128).rotate_left(64));
                    }
                }
            }
        }
        let mut acc = 0u128;
        fold(&self.home.log_dir, &mut acc);
        fold(&self.home.timers_dir(), &mut acc);
        fold(&self.home.temp_dir(), &mut acc);
        acc
    }

    /// Refresh only when something on disk changed since the last refresh.
    fn refresh_if_changed(&mut self) {
        let stamp = self.ledger_stamp();
        if stamp != self.ledger_stamp {
            self.ledger_stamp = stamp;
            self.refresh_all();
        }
    }

    fn switch_home(&mut self, path: &str) -> Result<(), String> {
        let p = std::path::PathBuf::from(path.trim());
        if !p.is_dir() {
            return Err(format!("{} is not a folder", p.display()));
        }
        let home = Home::new(p);
        if let Err(e) = home.save_as_default() {
            return Err(format!("could not remember the folder: {e}"));
        }
        self.home = home;
        self.log_selected = None;
        self.inv_selected = None;
        let ym = this_month();
        self.log_ym = ym;
        self.inv_ym = ym;
        self.rep_ym = ym;
        self.ledger_stamp = 0;
        self.refresh_all();
        Ok(())
    }

    /// Edits and deletes address rows by index. Another writer (CLI, Python
    /// TUI, a Claude Code session) may have changed the month since it was
    /// displayed, so re-read and confirm the row at `index` is the one shown.
    fn row_still_matches(&mut self, index: usize) -> Result<(), String> {
        let (y, m) = self.log_ym;
        let shown = self.log_entries.get(index).cloned();
        let now = store::read_month(&self.home, y, m).map_err(|e| e.to_string())?;
        if now.get(index) == shown.as_ref() {
            Ok(())
        } else {
            self.refresh_all();
            self.log_selected = None;
            Err("the ledger changed since this row was displayed; reloaded, select it again".into())
        }
    }

    fn set_status(&mut self, msg: impl Into<String>, color: Color32) {
        self.status = (msg.into(), color);
    }

    fn refresh_all(&mut self) {
        self.refresh_dashboard();
        self.refresh_log();
        self.refresh_invoice();
        self.refresh_report();
    }

    fn refresh_dashboard(&mut self) {
        self.timers = store::read_timers(&self.home).unwrap_or_default();
        self.summary = store::summary(&self.home, Local::now().date_naive()).ok();
        self.months = store::available_months(&self.home);
        let (y, m) = this_month();
        self.recent = store::read_month(&self.home, y, m).unwrap_or_default();
    }

    fn refresh_log(&mut self) {
        let (y, m) = self.log_ym;
        self.log_entries = store::read_month(&self.home, y, m).unwrap_or_default();
        if let Some(i) = self.log_selected {
            if i >= self.log_entries.len() {
                self.log_selected = None;
            }
        }
    }

    fn refresh_invoice(&mut self) {
        let (y, m) = self.inv_ym;
        self.inv_error.clear();
        match invoice::line_items_for(&self.home, y, m) {
            Ok((auto, raw)) => {
                self.inv_raw_total = raw;
                match invoice::read_staging(&self.home, y, m) {
                    Ok(Some(staged)) => {
                        self.inv_items = staged;
                        self.inv_staged = true;
                    }
                    _ => {
                        self.inv_items = auto;
                        self.inv_staged = false;
                    }
                }
            }
            Err(e) => {
                self.inv_items.clear();
                self.inv_raw_total = 0.0;
                self.inv_staged = false;
                self.inv_error = e.to_string();
            }
        }
        if let Some(i) = self.inv_selected {
            if i >= self.inv_items.len() {
                self.inv_selected = None;
            }
        }
        self.inv_next = invoice::peek_invoice_number(&self.home, &self.inv_client).ok();
    }

    fn refresh_report(&mut self) {
        let (y, m) = self.rep_ym;
        self.rep_entries = store::read_month(&self.home, y, m).unwrap_or_default();
        self.rep_year_entries = (1..=12).flat_map(|mm| store::read_month(&self.home, y, mm).unwrap_or_default()).collect();
    }

    fn persist_staging(&mut self) {
        let (y, m) = self.inv_ym;
        match invoice::write_staging(&self.home, &self.inv_items, y, m, Local::now().date_naive()) {
            Ok(p) => {
                self.inv_staged = true;
                self.set_status(format!("staging saved: {}", p.display()), OK);
            }
            Err(e) => self.set_status(format!("staging write failed: {e}"), ERR),
        }
    }

    // ── panes ───────────────────────────────────────────────────────────────

    fn ui_dashboard(&mut self, ui: &mut egui::Ui) {
        let now = Local::now().naive_local();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Quick start").strong());
            let resp = ui.add(egui::TextEdit::singleline(&mut self.quick_desc).hint_text("description (T123 / PR #45 refs group on the invoice)").desired_width(420.0));
            egui::ComboBox::from_id_salt("quick_cat").selected_text(&self.quick_cat).show_ui(ui, |ui| {
                for c in config::CATEGORIES {
                    ui.selectable_value(&mut self.quick_cat, c.to_string(), c);
                }
            });
            let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("▶ Start").clicked() || submit {
                self.quick_start();
            }
        });
        ui.add_space(6.0);

        ui.label(RichText::new("Running timers").strong());
        if self.timers.is_empty() {
            ui.label(RichText::new("No timers running.").weak());
        } else {
            let mut stop_idx = None;
            TableBuilder::new(ui)
                .striped(true)
                .column(Column::auto())
                .column(Column::auto())
                .column(Column::remainder())
                .column(Column::auto())
                .column(Column::auto())
                .column(Column::auto())
                .column(Column::auto())
                .header(20.0, |mut h| {
                    for t in ["#", "Cat", "Description", "Started", "Elapsed", "So far", ""] {
                        h.col(|ui| {
                            ui.label(RichText::new(t).strong());
                        });
                    }
                })
                .body(|mut body| {
                    for (i, t) in self.timers.iter().enumerate() {
                        body.row(22.0, |mut row| {
                            row.col(|ui| {
                                ui.label(t.timer_id.to_string());
                            });
                            row.col(|ui| {
                                ui.label(&t.category);
                            });
                            row.col(|ui| {
                                ui.label(&t.description);
                            });
                            row.col(|ui| {
                                ui.label(&t.start);
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(t.elapsed_hms(now)).monospace().color(ACCENT));
                            });
                            row.col(|ui| {
                                ui.label(money(t.live_amount(now)));
                            });
                            row.col(|ui| {
                                if ui.button("■ Stop").clicked() {
                                    stop_idx = Some(i);
                                }
                            });
                        });
                    }
                });
            if let Some(i) = stop_idx {
                let t = self.timers[i].clone();
                match engine::stop(&t.entry_date, &t.start, &t.category, &t.description, None, None) {
                    Ok(r) => self.stop_confirm = Some((t, r)),
                    Err(e) => self.set_status(format!("cannot stop #{}: {e}", t.timer_id), ERR),
                }
            }
        }

        ui.add_space(10.0);
        ui.separator();
        if let Some(s) = self.summary {
            ui.horizontal(|ui| {
                for (label, t) in [("Today", s.today), ("Week", s.week), ("Month", s.month)] {
                    ui.group(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(label).weak());
                            ui.label(RichText::new(format!("{:.2}h", t.hours)).heading());
                            ui.label(money(t.amount));
                        });
                    });
                }
            });
        }

        ui.add_space(10.0);
        ui.label(RichText::new("Recent entries").strong());
        let n = self.recent.len();
        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
            for e in self.recent.iter().rev().take(8) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&e.entry_date).weak());
                    ui.label(RichText::new(format!("{:>5}h", e.hrs)).monospace());
                    ui.label(RichText::new(&e.category).color(ACCENT));
                    ui.label(&e.description);
                    badge_label(ui, e);
                });
            }
            if n == 0 {
                ui.label(RichText::new("nothing logged this month yet").weak());
            }
        });
    }

    fn quick_start(&mut self) {
        let desc = self.quick_desc.trim().to_string();
        if desc.is_empty() {
            self.set_status("enter a description first", WARN);
            return;
        }
        match store::start_timer(&self.home, &desc, &self.quick_cat) {
            Ok(t) => {
                self.set_status(format!("▶ timer #{} started", t.timer_id), OK);
                self.quick_desc.clear();
                self.refresh_dashboard();
            }
            Err(e) => self.set_status(e.to_string(), ERR),
        }
    }

    fn month_nav(ui: &mut egui::Ui, id: &str, ym: &mut (i32, u32), months: &[(i32, u32)]) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            if ui.button("◀").clicked() {
                *ym = shift_month(*ym, -1);
                changed = true;
            }
            let label = format!("{:04}-{:02}", ym.0, ym.1);
            egui::ComboBox::from_id_salt(id).selected_text(RichText::new(label).strong()).show_ui(ui, |ui| {
                for &(y, m) in months {
                    if ui.selectable_label(*ym == (y, m), format!("{y:04}-{m:02}")).clicked() {
                        *ym = (y, m);
                        changed = true;
                    }
                }
            });
            if ui.button("▶").clicked() {
                *ym = shift_month(*ym, 1);
                changed = true;
            }
            if ui.button("today").clicked() {
                *ym = this_month();
                changed = true;
            }
        });
        changed
    }

    fn ui_log(&mut self, ui: &mut egui::Ui) {
        let months = self.months.clone();
        let mut ym = self.log_ym;
        let mut changed = false;
        ui.horizontal(|ui| {
            changed = Self::month_nav(ui, "log_month", &mut ym, &months);
            ui.separator();
            egui::ComboBox::from_id_salt("log_cat")
                .selected_text(if self.log_cat.is_empty() { "all categories" } else { &self.log_cat })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.log_cat, String::new(), "all categories");
                    for c in config::CATEGORIES {
                        ui.selectable_value(&mut self.log_cat, c.to_string(), c);
                    }
                });
            ui.add(egui::TextEdit::singleline(&mut self.log_search).hint_text("search descriptions").desired_width(240.0));
            ui.separator();
            if ui.button("+ Add").clicked() {
                self.entry_form = Some(EntryForm::blank(Local::now().date_naive()));
            }
            let sel = self.log_selected;
            if ui.add_enabled(sel.is_some(), egui::Button::new("Edit")).clicked() {
                if let Some(i) = sel {
                    self.entry_form = Some(EntryForm::from_entry(i, &self.log_entries[i]));
                }
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new("Delete")).clicked() {
                self.delete_confirm = sel;
            }
        });
        if changed {
            self.log_ym = ym;
            self.log_selected = None;
            self.refresh_log();
        }

        let needle = self.log_search.to_lowercase();
        let visible: Vec<usize> = self
            .log_entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.log_cat.is_empty() || e.category == self.log_cat)
            .filter(|(_, e)| needle.is_empty() || e.description.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect();
        let shown: Vec<&store::Entry> = visible.iter().map(|&i| &self.log_entries[i]).collect();
        let totals = store::sum(&shown.iter().map(|e| (*e).clone()).collect::<Vec<_>>());
        ui.label(RichText::new(format!("{} of {} entries · {:.2}h · {}", visible.len(), self.log_entries.len(), totals.hours, money(totals.amount))).weak());

        let mut clicked: Option<usize> = None;
        let mut dbl: Option<usize> = None;
        let selected = self.log_selected;
        egui::ScrollArea::vertical().show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .sense(egui::Sense::click())
                .column(Column::exact(84.0))
                .column(Column::exact(68.0))
                .column(Column::exact(68.0))
                .column(Column::exact(52.0))
                .column(Column::exact(76.0))
                .column(Column::remainder().clip(true))
                .column(Column::exact(64.0))
                .header(20.0, |mut h| {
                    for (t, right) in [("Date", false), ("Start", false), ("End", false), ("Hrs", true), ("Category", false), ("Description", false), ("", false)] {
                        h.col(|ui| head_cell(ui, t, right));
                    }
                })
                .body(|body| {
                    body.rows(20.0, visible.len(), |mut row| {
                        let idx = visible[row.index()];
                        let e = &self.log_entries[idx];
                        row.set_selected(selected == Some(idx));
                        row.col(|ui| {
                            ui.label(&e.entry_date);
                        });
                        row.col(|ui| {
                            ui.label(&e.start_time);
                        });
                        row.col(|ui| {
                            ui.label(&e.end_time);
                        });
                        row.col(|ui| num_cell(ui, &e.hrs));
                        row.col(|ui| {
                            ui.label(RichText::new(&e.category).color(ACCENT));
                        });
                        row.col(|ui| {
                            ui.add(egui::Label::new(&e.description).truncate());
                        });
                        row.col(|ui| badge_label(ui, e));
                        let r = row.response();
                        if r.clicked() {
                            clicked = Some(idx);
                        }
                        if r.double_clicked() {
                            dbl = Some(idx);
                        }
                    });
                });
        });
        if let Some(i) = clicked {
            self.log_selected = Some(i);
        }
        if let Some(i) = dbl {
            self.entry_form = Some(EntryForm::from_entry(i, &self.log_entries[i]));
        }
    }

    fn ui_invoice(&mut self, ui: &mut egui::Ui) {
        let months = self.months.clone();
        let mut ym = self.inv_ym;
        let mut changed = false;
        let mut client_changed = false;
        ui.horizontal(|ui| {
            changed = Self::month_nav(ui, "inv_month", &mut ym, &months);
            ui.separator();
            egui::ComboBox::from_id_salt("inv_client")
                .selected_text(self.home.client(&self.inv_client).map(|c| c.display_name.as_str()).unwrap_or("?"))
                .show_ui(ui, |ui| {
                    for c in self.home.clients.clone() {
                        if ui.selectable_label(self.inv_client == c.key, &c.display_name).clicked() {
                            self.inv_client = c.key.clone();
                            client_changed = true;
                        }
                    }
                });
            ui.separator();
            let sel = self.inv_selected;
            if ui.add_enabled(sel.is_some(), egui::Button::new("Edit line")).clicked() {
                if let Some(i) = sel {
                    let it = &self.inv_items[i];
                    self.line_form = Some(LineForm { index: i, label: it.label.clone(), hours: format!("{:.2}", it.hours), narrative: it.narrative.clone(), error: String::new() });
                }
            }
            if ui
                .add_enabled(sel.is_some() && self.inv_items.len() > 1, egui::Button::new("Drop line"))
                .on_disabled_hover_text("an invoice needs at least one line: a staging file with no rows reads as no staging")
                .clicked()
            {
                if let Some(i) = sel {
                    self.inv_items.remove(i);
                    self.inv_selected = None;
                    self.persist_staging();
                }
            }
            if ui.add_enabled(self.inv_staged, egui::Button::new("Reset to auto")).clicked() {
                let (y, m) = self.inv_ym;
                let _ = std::fs::remove_file(invoice::staging_path(&self.home, y, m));
                self.refresh_invoice();
                self.set_status("staging removed; showing auto-grouped items", ACCENT);
            }
        });
        if changed || client_changed {
            if changed {
                self.inv_ym = ym;
                self.inv_selected = None;
            }
            self.refresh_invoice();
        }

        if !self.inv_error.is_empty() {
            ui.label(RichText::new(&self.inv_error).color(WARN));
            return;
        }

        let client = self.home.client(&self.inv_client).cloned();
        let t = invoice::preview_totals(&self.inv_items, self.inv_raw_total);
        ui.horizontal(|ui| {
            ui.label(RichText::new(if self.inv_staged { "staged (edited)" } else { "auto-grouped" }).weak());
            ui.separator();
            ui.label(format!("raw {:.4}h  ${:.2}", t.raw_hours, t.raw_amount));
            ui.label(RichText::new(format!("invoice {:.2}h  ${:.2}", t.invoice_hours, t.invoice_amount)).strong());
            let d = if t.delta >= 0.0 { format!("Δ +${:.2}", t.delta) } else { format!("Δ -${:.2}", t.delta.abs()) };
            ui.label(RichText::new(d).color(if t.delta.abs() < 0.005 { OK } else { WARN }));
        });

        let mut clicked = None;
        let mut dbl = None;
        let selected = self.inv_selected;
        egui::ScrollArea::vertical().max_height(ui.available_height() - 70.0).show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .sense(egui::Sense::click())
                .column(Column::initial(120.0).at_least(80.0))
                .column(Column::exact(60.0))
                .column(Column::exact(80.0))
                .column(Column::remainder().clip(true))
                .header(20.0, |mut h| {
                    for (t, right) in [("Label", false), ("Hours", true), ("Total", true), ("Narrative", false)] {
                        h.col(|ui| head_cell(ui, t, right));
                    }
                })
                .body(|body| {
                    body.rows(20.0, self.inv_items.len(), |mut row| {
                        let i = row.index();
                        let it = &self.inv_items[i];
                        row.set_selected(selected == Some(i));
                        row.col(|ui| {
                            ui.label(RichText::new(&it.label).strong());
                        });
                        row.col(|ui| num_cell(ui, format!("{:.2}", it.hours)));
                        row.col(|ui| num_cell(ui, format!("${:.2}", it.total())));
                        row.col(|ui| {
                            ui.add(egui::Label::new(&it.narrative).truncate());
                        });
                        let r = row.response();
                        if r.clicked() {
                            clicked = Some(i);
                        }
                        if r.double_clicked() {
                            dbl = Some(i);
                        }
                    });
                });
        });
        if let Some(i) = clicked {
            self.inv_selected = Some(i);
        }
        if let Some(i) = dbl {
            let it = &self.inv_items[i];
            self.line_form = Some(LineForm { index: i, label: it.label.clone(), hours: format!("{:.2}", it.hours), narrative: it.narrative.clone(), error: String::new() });
        }

        ui.separator();
        ui.horizontal(|ui| {
            match client {
                Some(c) if c.html_capable => {
                    let next = self.inv_next;
                    let label = match next {
                        Some(n) => format!("Generate HTML invoice #{n}"),
                        None => "Generate HTML invoice".into(),
                    };
                    if ui.button(label).clicked() {
                        self.generate_html();
                    }
                    ui.label(RichText::new(format!("→ {}", invoice::out_path_for(&self.home, self.inv_ym.0, self.inv_ym.1).display())).weak());
                }
                Some(c) => {
                    let next = self.inv_next;
                    if ui.button(format!("Reserve invoice number{}", next.map(|n| format!(" #{n}")).unwrap_or_default())).clicked() {
                        match next {
                            Some(n) => match invoice::commit_invoice_number(&self.home, n, &self.inv_client) {
                                Ok(()) => self.set_status(format!("reserved #{n} for {}; build the {} invoice by hand", c.display_name, c.currency), OK),
                                Err(e) => self.set_status(e.to_string(), ERR),
                            },
                            None => self.set_status("counter unreadable", ERR),
                        }
                    }
                    ui.label(RichText::new(format!("{} invoices ({}) are hand-built; the tool only reserves the number.", c.display_name, c.currency)).weak());
                }
                None => {}
            }
        });
    }

    fn generate_html(&mut self) {
        let (y, m) = self.inv_ym;
        let entries = match invoice::load_entries(&self.home, y, m) {
            Ok(e) => e,
            Err(e) => {
                self.set_status(e.to_string(), ERR);
                return;
            }
        };
        let n = match invoice::next_invoice_number(&self.home, &self.inv_client) {
            Ok(n) => n,
            Err(e) => {
                self.set_status(e.to_string(), ERR);
                return;
            }
        };
        let html = match invoice::build_html(&self.home, &self.inv_items, &entries, y, m, n, &self.inv_client) {
            Ok(h) => h,
            Err(e) => {
                self.set_status(e.to_string(), ERR);
                return;
            }
        };
        let path = invoice::out_path_for(&self.home, y, m);
        if let Err(e) = std::fs::create_dir_all(path.parent().unwrap()).and_then(|_| std::fs::write(&path, html)) {
            self.set_status(format!("write failed: {e}"), ERR);
            return;
        }
        if let Err(e) = invoice::commit_invoice_number(&self.home, n, &self.inv_client) {
            self.set_status(format!("invoice written but counter not advanced: {e}"), ERR);
            return;
        }
        let opened = self.open_generated && open_in_browser(&path);
        self.set_status(
            format!("✓ invoice #{n} written: {}{}", path.display(), if opened { " (opened in browser; Print to PDF)" } else { " (open it in a browser and Print to PDF)" }),
            OK,
        );
    }

    fn ui_report(&mut self, ui: &mut egui::Ui) {
        let months = self.months.clone();
        let mut ym = self.rep_ym;
        if Self::month_nav(ui, "rep_month", &mut ym, &months) {
            self.rep_ym = ym;
            self.refresh_report();
        }
        let totals = report::month_totals(&self.rep_entries);
        ui.label(RichText::new(format!("{} entries · {:.2}h · {}", self.rep_entries.len(), totals.hours, money(totals.amount))).weak());
        ui.add_space(6.0);
        let dark = self.dark;
        let (year, month) = self.rep_ym;

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |cols| {
                cols[0].label(RichText::new("By category").strong());
                let cats = report::by_category(&self.rep_entries);
                TableBuilder::new(&mut cols[0])
                    .striped(true)
                    .column(Column::initial(90.0).at_least(70.0))
                    .column(Column::exact(58.0))
                    .column(Column::exact(64.0))
                    .column(Column::exact(80.0))
                    .column(Column::exact(52.0))
                    .column(Column::remainder())
                    .header(20.0, |mut h| {
                        for (t, right) in [("Category", false), ("Entries", true), ("Hours", true), ("Amount", true), ("Share", true), ("", false)] {
                            h.col(|ui| head_cell(ui, t, right));
                        }
                    })
                    .body(|mut body| {
                        for c in &cats {
                            body.row(20.0, |mut row| {
                                row.col(|ui| {
                                    ui.label(RichText::new(&c.category).color(ACCENT));
                                });
                                row.col(|ui| num_cell(ui, c.entries.to_string()));
                                row.col(|ui| num_cell(ui, format!("{:.2}", c.hours)));
                                row.col(|ui| num_cell(ui, money(c.amount)));
                                row.col(|ui| num_cell(ui, format!("{}%", c.share_pct)));
                                row.col(|_| {});
                            });
                        }
                    });

                cols[1].label(RichText::new("By week (Mon–Sun)").strong());
                let weeks = report::by_week(&self.rep_entries);
                TableBuilder::new(&mut cols[1])
                    .striped(true)
                    .column(Column::initial(90.0).at_least(80.0))
                    .column(Column::exact(58.0))
                    .column(Column::exact(64.0))
                    .column(Column::exact(80.0))
                    .column(Column::remainder())
                    .header(20.0, |mut h| {
                        for (t, right) in [("Week", false), ("Entries", true), ("Hours", true), ("Amount", true), ("", false)] {
                            h.col(|ui| head_cell(ui, t, right));
                        }
                    })
                    .body(|mut body| {
                        for w in &weeks {
                            body.row(20.0, |mut row| {
                                row.col(|ui| {
                                    ui.label(w.span());
                                });
                                row.col(|ui| num_cell(ui, w.entries.to_string()));
                                row.col(|ui| num_cell(ui, format!("{:.2}", w.hours)));
                                row.col(|ui| num_cell(ui, money(w.amount)));
                                row.col(|_| {});
                            });
                        }
                    });
            });

            ui.add_space(14.0);
            ui.separator();

            // ── Activity heatmaps ───────────────────────────────────────────
            let month_name = NaiveDate::from_ymd_opt(year, month, 1).map(|d| d.format("%B %Y").to_string()).unwrap_or_default();
            ui.label(RichText::new(format!("Time of day · {month_name}")).strong());
            ui.label(RichText::new("billed hours spread over the clock hours each stopwatch session covered; manual entries have no clock position").weak().small());
            let hod = report::hour_of_day_hours(&self.rep_entries);
            let hod_labels: Vec<String> = (0..24).map(|h| format!("{h:02}")).collect();
            heat_strip(ui, &hod, &hod_labels, 3, dark, |i| format!("{:02}:00–{:02}:00  {}", i, (i + 1) % 24, fmt_h(hod[i])));

            ui.add_space(8.0);
            ui.label(RichText::new(format!("Day of week · {month_name}")).strong());
            let wd = report::weekday_hours(&self.rep_entries);
            let wd_labels: Vec<String> = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().map(|s| s.to_string()).collect();
            heat_strip(ui, &wd, &wd_labels, 1, dark, |i| format!("{}  {}", wd_labels[i], fmt_h(wd[i])));

            ui.add_space(8.0);
            let year_total: Decimal = self.rep_year_entries.iter().map(store::Entry::hours).sum();
            ui.label(RichText::new(format!("Calendar · {year}  ({} entries, {:.2}h)", self.rep_year_entries.len(), year_total)).strong());
            let daily = report::daily_totals(&self.rep_year_entries);
            heat_calendar(ui, year, &daily, dark);
            ui.label(RichText::new("colour scale is relative to the busiest day of the year; hover a cell for the date").weak().small());
        });
    }

    // ── modals ──────────────────────────────────────────────────────────────

    fn modal_stop(&mut self, ctx: &egui::Context) {
        let Some((t, r)) = self.stop_confirm.clone() else { return };
        let mut close = false;
        let mut confirm = false;
        egui::Modal::new(egui::Id::new("stop_modal")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading(format!("Stop timer #{}?", t.timer_id));
            ui.label(format!("[{}] {}", r.category, r.description));
            ui.label(format!("{} → {}", r.start_time, r.end_time));
            ui.label(RichText::new(format!("Duration: {}h   Billable: ${}", r.hrs, r.billable())).strong());
            if r.rounded() {
                ui.label(RichText::new(format!("+¼ quarter-hour rule: measured {}h rounded UP to the next 0.25h mark = {}h billed.", r.measured_hrs, r.hrs)).color(WARN));
            }
            if r.reboot_capped {
                ui.label(RichText::new(format!("⚡ spanned a reboot: end capped at first boot ({}).", r.end_time)).color(WARN));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("■ Stop and log").clicked() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if confirm {
            match store::stop_timer(&self.home, &t, Some(&r.end_time)) {
                Ok(res) => self.set_status(format!("■ timer #{} stopped: {}h (${})", t.timer_id, res.hrs, res.billable()), OK),
                Err(e) => self.set_status(e.to_string(), ERR),
            }
            self.stop_confirm = None;
            self.refresh_all();
        } else if close {
            self.stop_confirm = None;
        }
    }

    fn modal_entry(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.entry_form.clone() else { return };
        let mut save = false;
        let mut close = false;
        egui::Modal::new(egui::Id::new("entry_modal")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(if f.index.is_some() { "Edit entry" } else { "Add entry" });
            egui::Grid::new("entry_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Date");
                ui.text_edit_singleline(&mut f.date);
                ui.end_row();
                ui.label("Start / End");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut f.start).desired_width(90.0));
                    ui.add(egui::TextEdit::singleline(&mut f.end).desired_width(90.0));
                    ui.label(RichText::new("HH:MM:SS or `manual`").weak());
                });
                ui.end_row();
                ui.label("Hours");
                ui.add(egui::TextEdit::singleline(&mut f.hrs).desired_width(90.0));
                ui.end_row();
                ui.label("Category");
                egui::ComboBox::from_id_salt("form_cat").selected_text(&f.category).show_ui(ui, |ui| {
                    for c in config::CATEGORIES {
                        ui.selectable_value(&mut f.category, c.to_string(), c);
                    }
                });
                ui.end_row();
                ui.label("Description");
                ui.add(egui::TextEdit::singleline(&mut f.description).desired_width(400.0));
                ui.end_row();
            });
            if !f.error.is_empty() {
                ui.label(RichText::new(&f.error).color(ERR));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() || ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S)) {
                    save = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        });
        if save {
            match f.to_entry() {
                Ok(e) => {
                    let (y, m) = self.log_ym;
                    let res: Result<(), String> = match f.index {
                        Some(i) => self
                            .row_still_matches(i)
                            .and_then(|_| store::update_entry(&self.home, y, m, i, e).map(|_| ()).map_err(|e| e.to_string())),
                        None => store::insert_entry(&self.home, e).map(|_| ()).map_err(|e| e.to_string()),
                    };
                    match res {
                        Ok(()) => {
                            self.set_status(if f.index.is_some() { "entry updated" } else { "entry added" }, OK);
                            self.entry_form = None;
                            self.refresh_all();
                        }
                        Err(err) => {
                            f.error = err.to_string();
                            self.entry_form = Some(f);
                        }
                    }
                }
                Err(err) => {
                    f.error = err;
                    self.entry_form = Some(f);
                }
            }
        } else if close {
            self.entry_form = None;
        } else {
            self.entry_form = Some(f);
        }
    }

    fn modal_delete(&mut self, ctx: &egui::Context) {
        let Some(i) = self.delete_confirm else { return };
        let Some(e) = self.log_entries.get(i).cloned() else {
            self.delete_confirm = None;
            return;
        };
        let mut yes = false;
        let mut no = false;
        egui::Modal::new(egui::Id::new("delete_modal")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("Delete entry?");
            ui.label(format!("{}  {}h  [{}] {}", e.entry_date, e.hrs, e.category, e.description));
            ui.label(RichText::new("The monthly CSV is rewritten atomically; there is no undo.").weak());
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Delete").color(ERR)).clicked() {
                    yes = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    no = true;
                }
            });
        });
        if yes {
            let (y, m) = self.log_ym;
            let res = self
                .row_still_matches(i)
                .and_then(|_| store::delete_entry(&self.home, y, m, i).map(|_| ()).map_err(|e| e.to_string()));
            match res {
                Ok(()) => self.set_status("entry deleted", OK),
                Err(err) => self.set_status(err, ERR),
            }
            self.delete_confirm = None;
            self.log_selected = None;
            self.refresh_all();
        } else if no {
            self.delete_confirm = None;
        }
    }

    fn modal_line(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.line_form.clone() else { return };
        let mut save = false;
        let mut close = false;
        egui::Modal::new(egui::Id::new("line_modal")).show(ctx, |ui| {
            ui.set_width(600.0);
            ui.heading("Edit line item");
            egui::Grid::new("line_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Label");
                ui.add(egui::TextEdit::singleline(&mut f.label).desired_width(300.0));
                ui.end_row();
                ui.label("Hours");
                ui.add(egui::TextEdit::singleline(&mut f.hours).desired_width(90.0));
                ui.end_row();
                ui.label("Narrative");
                ui.add(egui::TextEdit::multiline(&mut f.narrative).desired_width(460.0).desired_rows(3));
                ui.end_row();
            });
            ui.label(RichText::new("Rate is fixed at $16.00/hr. Edits are saved to temp/staging-YYYY-MM.txt.").weak());
            if !f.error.is_empty() {
                ui.label(RichText::new(&f.error).color(ERR));
            }
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        });
        if save {
            match f.hours.trim().parse::<f64>() {
                Ok(h) if h >= 0.0 && f.label.trim().len() > 0 => {
                    if let Some(it) = self.inv_items.get_mut(f.index) {
                        it.label = f.label.trim().replace(" | ", " / ");
                        it.hours = h;
                        it.narrative = f.narrative.trim().replace('\n', " ").replace(" | ", " / ");
                    }
                    self.line_form = None;
                    self.persist_staging();
                }
                _ => {
                    f.error = "label required; hours must be a non-negative number".into();
                    self.line_form = Some(f);
                }
            }
        } else if close {
            self.line_form = None;
        } else {
            self.line_form = Some(f);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }
}

impl App {
    /// One full frame: tick, key handling, chrome, active pane, modals.
    fn frame(&mut self, ctx: &egui::Context) {
        // Poll the ledger once a second: cheap, and it makes CLI / Claude Code
        // writes appear without a daemon or a file watcher (which is unreliable
        // over WSL's 9P mounts anyway).
        if self.last_tick.elapsed() >= TICK {
            self.last_tick = Instant::now();
            self.refresh_if_changed();
        }
        ctx.request_repaint_after(TICK);

        ctx.input(|i| {
            if i.modifiers.ctrl && i.key_pressed(egui::Key::T) {
                self.dark = !self.dark;
            }
            for (n, p) in Pane::ALL.iter().enumerate() {
                let key = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4][n];
                if i.modifiers.ctrl && i.key_pressed(key) {
                    self.pane = *p;
                }
            }
        });
        ctx.set_theme(if self.dark { egui::Theme::Dark } else { egui::Theme::Light });

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("stint").strong().color(ACCENT));
                if self.home.demo {
                    ui.label(RichText::new("DEMO").color(WARN).strong());
                }
                ui.separator();
                for p in Pane::ALL {
                    if ui.selectable_label(self.pane == p, p.title()).clicked() {
                        self.pane = p;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(Local::now().format("%H:%M:%S").to_string()).monospace());
                    if let Some(s) = self.summary {
                        ui.label(RichText::new(format!("today {:.2}h  ·  week {:.2}h  ·  month {:.2}h {}", s.today.hours, s.week.hours, s.month.hours, money(s.month.amount))).weak());
                    }
                    if ui.small_button(if self.dark { "light" } else { "dark" }).on_hover_text("toggle theme (ctrl+t)").clicked() {
                        self.dark = !self.dark;
                    }
                });
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button("ledger…").on_hover_text("choose the folder holding stint-YYYY-MM.csv and .timers/").clicked() {
                    self.home_form = Some((self.home.log_dir.display().to_string(), String::new()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("ctrl+1..4 panes · ctrl+t theme · double-click a row to edit").weak().small());
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(&self.status.0).color(self.status.1)).truncate());
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(self.home.log_dir.display().to_string()).weak().small()).truncate());
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.pane {
            Pane::Dashboard => self.ui_dashboard(ui),
            Pane::Log => self.ui_log(ui),
            Pane::Invoice => self.ui_invoice(ui),
            Pane::Report => self.ui_report(ui),
        });

        self.modal_stop(ctx);
        self.modal_entry(ctx);
        self.modal_delete(ctx);
        self.modal_line(ctx);
        self.modal_home(ctx);
    }

    fn modal_home(&mut self, ctx: &egui::Context) {
        let Some((mut path, mut error)) = self.home_form.clone() else { return };
        let mut save = false;
        let mut close = false;
        let first_run = !self.home.looks_like_ledger();
        egui::Modal::new(egui::Id::new("home_modal")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.heading("Ledger folder");
            if first_run {
                ui.label(RichText::new(format!("No ledger found at {}.", self.home.log_dir.display())).color(WARN));
            }
            ui.label("The folder holding stint-YYYY-MM.csv, .timers/ and temp/. On Windows this can be the WSL repo, for example:");
            ui.label(RichText::new(r"\\wsl.localhost\Ubuntu\home\<user>\repos\stint").monospace());
            ui.add(egui::TextEdit::singleline(&mut path).desired_width(600.0));
            if !error.is_empty() {
                ui.label(RichText::new(&error).color(ERR));
            }
            ui.label(RichText::new("Remembered in the per-user config; --home, $STINT_LOG_DIR and $STINT_HOME still take precedence.").weak().small());
            ui.horizontal(|ui| {
                if ui.button("Use this folder").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    save = true;
                }
                if !first_run && (ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape))) {
                    close = true;
                }
            });
        });
        if save {
            match self.switch_home(&path) {
                Ok(()) => {
                    self.set_status(format!("ledger: {}", self.home.log_dir.display()), OK);
                    self.home_form = None;
                }
                Err(e) => {
                    error = e;
                    self.home_form = Some((path, error));
                }
            }
        } else if close {
            self.home_form = None;
        } else {
            self.home_form = Some((path, error));
        }
    }
}

/// Launch the window. Returns Err if no window/GL context could be created,
/// which the caller turns into the TUI fallback.
pub fn run(home: Home, pane: &str) -> Result<(), String> {
    let start_pane = Pane::parse(pane).ok_or_else(|| format!("unknown pane {pane:?} (dashboard, log, invoice, report)"))?;
    let title = if home.demo { "stint (demo)" } else { "stint" };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1120.0, 720.0]).with_min_inner_size([760.0, 480.0]).with_title(title),
        // No eframe storage: nothing to persist, and a restored window
        // geometry from an interrupted run once came back as 79x101 at 0,0.
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "stint",
        options,
        Box::new(move |cc| {
            let mut app = App::new(cc, home);
            app.pane = start_pane;
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    //! Headless smoke tests: egui renders frames without a window, so every
    //! pane and modal runs through its real layout code against the sample
    //! ledger. These catch panics (index out of range after a delete, a
    //! missing template) that only a real click would otherwise reveal.

    use super::*;

    fn sample_home() -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().unwrap();
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let src = repo.join("samples");
        for entry in std::fs::read_dir(&src).unwrap().flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "csv") {
                std::fs::copy(&p, dir.path().join(p.file_name().unwrap())).unwrap();
            }
        }
        std::fs::create_dir_all(dir.path().join("temp")).unwrap();
        std::fs::copy(src.join("temp/invoice-dynamic.html"), dir.path().join("temp/invoice-dynamic.html")).unwrap();
        let mut home = Home::new(dir.path());
        home.demo = true; // opens on the newest sample month
        (dir, home)
    }

    fn run_frame(ctx: &egui::Context, app: &mut App) {
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.frame(ctx));
    }

    #[test]
    fn every_pane_renders_on_the_sample_ledger() {
        let (_d, home) = sample_home();
        let ctx = egui::Context::default();
        let mut app = App::with_ctx(&ctx, home);
        assert_eq!(app.log_ym, (2026, 6));
        assert!(!app.log_entries.is_empty(), "sample month should load");
        assert!(!app.inv_items.is_empty(), "invoice items should group");
        for pane in Pane::ALL {
            app.pane = pane;
            run_frame(&ctx, &mut app);
            run_frame(&ctx, &mut app);
        }
    }

    #[test]
    fn quick_start_then_stop_modal_logs_a_row() {
        let (_d, home) = sample_home();
        let ctx = egui::Context::default();
        let mut app = App::with_ctx(&ctx, home.clone());
        app.quick_desc = "T99 headless timer".into();
        app.quick_cat = "pr".into();
        app.quick_start();
        assert_eq!(app.timers.len(), 1);
        let t = app.timers[0].clone();
        let r = engine::stop(&t.entry_date, &t.start, &t.category, &t.description, None, Some(&[])).unwrap();
        app.stop_confirm = Some((t.clone(), r));
        run_frame(&ctx, &mut app); // modal renders
        // Confirm through the same call the modal makes.
        let res = store::stop_timer(&home, &t, None).unwrap();
        app.stop_confirm = None;
        app.refresh_all();
        assert_eq!(res.hrs, "0.25");
        assert!(app.timers.is_empty());
        let (y, m) = this_month();
        let rows = store::read_month(&home, y, m).unwrap();
        assert!(rows.iter().any(|e| e.description == "T99 headless timer"));
        run_frame(&ctx, &mut app);
    }

    #[test]
    fn entry_form_validates_and_saves() {
        let (_d, home) = sample_home();
        let ctx = egui::Context::default();
        let mut app = App::with_ctx(&ctx, home.clone());
        let mut f = EntryForm::blank(NaiveDate::from_ymd_opt(2026, 6, 15).unwrap());
        f.description = "headless add".into();
        f.hrs = "abc".into();
        assert!(f.to_entry().is_err());
        f.hrs = "1.5".into();
        let e = f.to_entry().unwrap();
        assert_eq!(e.hrs, "1.5");
        let before = app.log_entries.len();
        store::insert_entry(&home, e).unwrap();
        app.refresh_log();
        assert_eq!(app.log_entries.len(), before + 1);
        // Edit the last row through the form, then delete it, rendering between.
        let last = app.log_entries.len() - 1;
        app.entry_form = Some(EntryForm::from_entry(last, &app.log_entries[last]));
        run_frame(&ctx, &mut app);
        app.entry_form = None;
        app.delete_confirm = Some(last);
        run_frame(&ctx, &mut app);
        store::delete_entry(&home, 2026, 6, last).unwrap();
        app.delete_confirm = None;
        app.refresh_all();
        assert_eq!(app.log_entries.len(), before);
        run_frame(&ctx, &mut app);
    }

    #[test]
    fn invoice_edit_persists_staging_and_generates_html() {
        let (_d, home) = sample_home();
        let ctx = egui::Context::default();
        let mut app = App::with_ctx(&ctx, home.clone());
        app.open_generated = false; // never launch a browser from a test
        app.pane = Pane::Invoice;
        assert!(!app.inv_staged);
        app.inv_items[0].hours = 9.75;
        app.persist_staging();
        assert!(app.inv_staged);
        assert!(invoice::staging_path(&home, 2026, 6).exists());
        app.refresh_invoice();
        assert_eq!(app.inv_items[0].hours, 9.75, "staged edit survives a refresh");
        run_frame(&ctx, &mut app);
        // Drop a line, then generate (placeholder counter seeds at 1001 and advances).
        app.inv_items.remove(app.inv_items.len() - 1);
        app.persist_staging();
        app.generate_html();
        assert!(invoice::out_path_for(&home, 2026, 6).exists(), "{}", app.status.0);
        assert_eq!(std::fs::read_to_string(home.log_dir.join(".invoice-counter")).unwrap().trim(), "1002");
        // A hand-built client only reserves.
        app.inv_client = app.home.manual_client().unwrap().key.clone();
        app.generate_html();
        assert!(app.status.0.contains("not HTML-capable"), "{}", app.status.0);
        run_frame(&ctx, &mut app);
    }

    #[test]
    fn month_navigation_wraps_years() {
        assert_eq!(shift_month((2026, 1), -1), (2025, 12));
        assert_eq!(shift_month((2026, 12), 1), (2027, 1));
        assert_eq!(shift_month((2026, 6), -18), (2024, 12));
    }
}
