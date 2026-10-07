//! `stint`: the compiled front-end over `stint-core`.
//!
//! The CLI verbs mirror the Bash `stint.sh` script one for one (same messages, so a
//! Claude Code session driving either sees the same output), plus `--json` for
//! machine consumers. `tui`, `gui` and `mcp` subcommands land here later.

use std::path::PathBuf;

use anyhow::{bail, Result};
use chrono::{Datelike, Local};
use clap::{Args, Parser, Subcommand};
use stint_core::{config, engine, invoice, store, Home};

#[cfg(feature = "gui")]
mod gui;
#[cfg(feature = "mcp")]
mod mcp;

#[derive(Parser)]
#[command(name = "stint", version, about = "Time tracker + invoice generator")]
struct Cli {
    /// Ledger directory (default: $STINT_LOG_DIR, $STINT_HOME, ~/repos/stint).
    #[arg(long, global = true)]
    home: Option<PathBuf>,

    /// Emit machine-readable JSON instead of the human report.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a timer: stint start "description" [category]
    Start {
        description: String,
        #[arg(default_value = config::DEFAULT_CATEGORY)]
        category: String,
    },
    /// Stop a timer (id required if several are running)
    Stop { id: Option<u32> },
    /// Stop every running timer
    StopAll,
    /// Show running timers
    Status,
    /// Formatted table for a month: stint log [YYYY MM]
    Log { year: Option<i32>, month: Option<u32> },
    /// Manual entry: stint add "description" category hours [YYYY-MM-DD]
    Add {
        description: String,
        category: String,
        hours: String,
        date: Option<String>,
    },
    /// Today / week / month totals
    Summary,
    /// Group + round a month into invoice line items; writes temp/staging-YYYY-MM.txt
    Consolidate { year: Option<i32>, month: Option<u32> },
    /// Invoice for a month: summary (default) or `--html` render into temp/
    Invoice {
        year: Option<i32>,
        month: Option<u32>,
        /// Render temp/invoice-YYYY-MM.html and advance the client's counter
        #[arg(long)]
        html: bool,
        /// Client key from clients.json (default: its default_client)
        #[arg(long)]
        client: Option<String>,
    },
    /// Open the desktop app (falls back to the TUI when no display is available)
    Gui {
        /// Fail instead of falling back to the TUI
        #[arg(long)]
        no_fallback: bool,
        /// Pane to open on: dashboard, log, invoice, report
        #[arg(long, default_value = "dashboard")]
        pane: String,
    },
    /// Full-screen terminal app (currently the Python Textual TUI via uv)
    Tui,
    /// Serve the ledger as MCP tools over stdio (register: `claude mcp add stint -- stint mcp`)
    Mcp,
    /// Print the resolved ledger location
    Home,
    /// Parity probes for the pytest bash-oracle harness (hidden)
    #[command(hide = true)]
    Parity(ParityArgs),
}

#[derive(Args)]
struct ParityArgs {
    #[command(subcommand)]
    probe: Probe,
}

#[derive(Subcommand)]
enum Probe {
    /// CSV row for a stop with no reboot cap (oracle.sh row)
    Row { entry_date: String, start: String, end: String, category: String, description: String },
    /// Measured hours, 2dp (oracle.sh hrs)
    Hrs { start: String, end: String },
    /// Billable for an hours figure (oracle.sh bill)
    Bill { hrs: String },
    /// Line items for a month as JSON (invoice parity)
    Items { year: i32, month: u32 },
    /// Staging text for a month on stdout, fixed date (invoice parity)
    Staging { year: i32, month: u32 },
    /// Rendered HTML on stdout for a pinned invoice number (invoice parity)
    Html { year: i32, month: u32, inv_num: u32 },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("stint: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let home = Home::resolve(cli.home.as_deref());
    let json = cli.json;

    match cli.cmd {
        Cmd::Start { description, category } => cmd_start(&home, description, category, json),
        Cmd::Stop { id } => cmd_stop(&home, id, json),
        Cmd::StopAll => cmd_stop_all(&home, json),
        Cmd::Status => cmd_status(&home, json),
        Cmd::Log { year, month } => cmd_log(&home, year, month, json),
        Cmd::Add { description, category, hours, date } => cmd_add(&home, description, category, hours, date, json),
        Cmd::Summary => cmd_summary(&home, json),
        Cmd::Consolidate { year, month } => cmd_consolidate(&home, year, month, json),
        Cmd::Invoice { year, month, html, client } => {
            let client = client.unwrap_or_else(|| home.default_client.clone());
            cmd_invoice(&home, year, month, html, &client, json)
        }
        Cmd::Gui { no_fallback, pane } => cmd_gui(&home, no_fallback, &pane),
        Cmd::Tui => cmd_tui(&home),
        Cmd::Mcp => cmd_mcp(&home),
        Cmd::Home => {
            if json {
                println!("{}", serde_json::json!({ "log_dir": home.log_dir, "demo": home.demo }));
            } else {
                println!("{}{}", home.log_dir.display(), if home.demo { "  (demo)" } else { "" });
            }
            Ok(())
        }
        Cmd::Parity(p) => cmd_parity(&home, p),
    }
}

fn cmd_start(home: &Home, mut description: String, mut category: String, json: bool) -> Result<()> {
    // Order-tolerant like Bash cmd_start: `start <category> "description"` is a
    // common slip; swap when arg1 is a category and arg2 isn't.
    if config::is_category(&description) && !config::is_category(&category) {
        std::mem::swap(&mut description, &mut category);
    }
    if description.trim().is_empty() {
        bail!("Usage: stint start \"description\" [category]\nCategories: {}", config::CATEGORIES.join(", "));
    }
    let t = store::start_timer(home, &description, &category)?;
    if json {
        println!("{}", serde_json::to_string(&t)?);
    } else {
        println!("▶  Timer #{} started: [{}] {}", t.timer_id, t.category, t.description);
        println!("   Started at: {}", t.start);
    }
    Ok(())
}

fn cmd_stop(home: &Home, id: Option<u32>, json: bool) -> Result<()> {
    let timer = match id {
        Some(id) => store::find_timer(home, id)?,
        None => {
            let timers = store::read_timers(home)?;
            match timers.len() {
                0 => bail!("No timers running. Use 'stint start' first."),
                1 => timers.into_iter().next().unwrap(),
                _ => {
                    if json {
                        let ids: Vec<u32> = timers.iter().map(|t| t.timer_id).collect();
                        println!("{}", serde_json::json!({ "error": "multiple timers running; specify an id", "running": ids }));
                    } else {
                        eprintln!("Multiple timers running — specify an ID: stint stop <id>\n");
                        cmd_status(home, false)?;
                    }
                    std::process::exit(1);
                }
            }
        }
    };
    let result = store::stop_timer(home, &timer, None)?;
    report_stop(timer.timer_id, &result, json)?;
    Ok(())
}

fn cmd_stop_all(home: &Home, json: bool) -> Result<()> {
    let timers = store::read_timers(home)?;
    if timers.is_empty() {
        println!("No timers running.");
        return Ok(());
    }
    let mut results = Vec::new();
    for t in &timers {
        let r = store::stop_timer(home, t, None)?;
        if !json {
            report_stop(t.timer_id, &r, false)?;
            println!();
        }
        results.push(r);
    }
    if json {
        println!("{}", serde_json::to_string(&results)?);
    }
    Ok(())
}

/// Same lines as Bash `_stop_timer_file`, including the quarter-hour
/// explanation aimed at Claude Code sessions in other repos.
fn report_stop(id: u32, r: &engine::StopResult, json: bool) -> Result<()> {
    if json {
        let mut v = serde_json::to_value(r)?;
        v["timer_id"] = id.into();
        v["billable"] = r.billable().into();
        v["csv_row"] = r.csv_row().into();
        println!("{v}");
        return Ok(());
    }
    if r.reboot_capped {
        println!("⚠  Timer #{id} spanned a reboot — end capped at first boot ({}).", r.end_time);
        println!("   (stint timers are files and survive reboots; edit the CSV if the real stop differs.)");
    }
    println!("■  Timer #{id} stopped.");
    println!("   [{}] {}", r.category, r.description);
    println!("   {} → {}", r.start_time, r.end_time);
    println!("   Duration: {}h  |  Billable: ${}", r.hrs, r.billable());
    if r.rounded() {
        println!("   ⚡ quarter-hour rule: measured {}h rounded UP to next 0.25h mark = {}h billed.", r.measured_hrs, r.hrs);
        println!("      Intended billing convention (15-min billing granularity), NOT a bug — do not \"fix\" it.");
    }
    Ok(())
}

fn cmd_status(home: &Home, json: bool) -> Result<()> {
    let timers = store::read_timers(home)?;
    if json {
        let now = Local::now().naive_local();
        let rows: Vec<_> = timers
            .iter()
            .map(|t| {
                let mut v = serde_json::to_value(t).unwrap_or_default();
                v["elapsed_seconds"] = t.elapsed_seconds(now).into();
                v["elapsed_hms"] = t.elapsed_hms(now).into();
                v
            })
            .collect();
        println!("{}", serde_json::Value::Array(rows));
        return Ok(());
    }
    if timers.is_empty() {
        println!("No timers running.");
        return Ok(());
    }
    let now = Local::now().naive_local();
    for t in &timers {
        println!("⏱  #{} Running: [{}] {}", t.timer_id, t.category, t.description);
        println!("   Started: {}", t.start);
        println!("   Elapsed: {}m", t.elapsed_seconds(now) / 60);
    }
    Ok(())
}

fn cmd_log(home: &Home, year: Option<i32>, month: Option<u32>, json: bool) -> Result<()> {
    let today = Local::now().date_naive();
    let (year, month) = (year.unwrap_or(today.year()), month.unwrap_or(today.month()));
    let entries = store::read_month(home, year, month)?;
    if json {
        println!("{}", serde_json::to_string(&entries)?);
        return Ok(());
    }
    if !home.csv_path(year, month).exists() {
        println!("No log found for {year}-{month:02}.");
        return Ok(());
    }
    let totals = store::sum(&entries);
    println!();
    println!("═══════════════════════════════════════════════════");
    println!("  Time Log — {year}-{month:02}");
    println!("═══════════════════════════════════════════════════");
    println!("{:<12} {:<8} {:<8} {:<6} {:<10} {}", "Date", "Start", "End", "Hrs", "Category", "Description");
    println!("───────────────────────────────────────────────────");
    for e in &entries {
        println!("{:<12} {:<8} {:<8} {:<6} {:<10} {}", e.entry_date, e.start_time, e.end_time, e.hrs, e.category, e.description);
    }
    println!("───────────────────────────────────────────────────");
    println!("  Total hours : {:.2}h", totals.hours);
    println!("  Rate        : ${}/hr", config::RATE);
    println!("  Total due   : ${:.2} USD", totals.amount);
    println!("═══════════════════════════════════════════════════");
    println!();
    Ok(())
}

fn cmd_add(home: &Home, description: String, category: String, hours: String, date: Option<String>, json: bool) -> Result<()> {
    if description.trim().is_empty() || category.trim().is_empty() || hours.trim().is_empty() {
        bail!("Usage: stint add \"description\" category hours [YYYY-MM-DD]");
    }
    // Hours go into the CSV verbatim, so anything bc wouldn't accept (`1,5`,
    // `abc`) would corrupt the column layout or silently sum as 0.
    let parsed: rust_decimal::Decimal = hours
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("hours must be a decimal number like 1.5 (got {hours:?})"))?;
    if parsed.is_sign_negative() {
        bail!("hours must not be negative (got {hours:?})");
    }
    if !config::is_category(&category) {
        eprintln!("stint: note: {category:?} is not a known category ({})", config::CATEGORIES.join(", "));
    }
    let e = store::add_entry(home, &description, &category, hours.trim(), date.as_deref())?;
    if json {
        println!("{}", serde_json::to_string(&e)?);
    } else {
        println!("✓ Added: [{}] {} — {}h (${})", e.category, e.description, e.hrs, engine::billable_for_hrs(&e.hrs));
    }
    Ok(())
}

fn cmd_summary(home: &Home, json: bool) -> Result<()> {
    let s = store::summary(home, Local::now().date_naive())?;
    if json {
        println!("{}", serde_json::to_string(&s)?);
    } else {
        println!("Today : {:>7.2}h  ${:.2}", s.today.hours, s.today.amount);
        println!("Week  : {:>7.2}h  ${:.2}", s.week.hours, s.week.amount);
        println!("Month : {:>7.2}h  ${:.2}", s.month.hours, s.month.amount);
    }
    Ok(())
}

/// True when a GUI window can plausibly be opened on this machine.
fn display_available() -> bool {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        return true;
    }
    ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
}

/// Under WSLg, Mesa's loader tries the d3d12/zink paths first and fails
/// ("ZINK: failed to choose pdev", "failed to create dri2 screen"), leaving a
/// window with no GL context and nothing drawn. Forcing the software
/// rasteriser (llvmpipe) gives a working, fast-enough egui. Explicitly set
/// values are respected, so `LIBGL_ALWAYS_SOFTWARE=0` opts out.
fn apply_wslg_gl_defaults() {
    if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_some() {
        return;
    }
    let wsl = std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/version").is_ok_and(|v| v.to_lowercase().contains("microsoft"));
    if wsl {
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    }
}

fn cmd_gui(home: &Home, no_fallback: bool, pane: &str) -> Result<()> {
    #[cfg(not(feature = "gui"))]
    let _ = pane;
    #[cfg(feature = "gui")]
    {
        apply_wslg_gl_defaults();
        if display_available() {
            match gui::run(home.clone(), pane) {
                Ok(()) => return Ok(()),
                Err(e) if no_fallback => bail!("GUI failed to start: {e}"),
                Err(e) => eprintln!("stint: GUI failed to start ({e}); falling back to the TUI"),
            }
        } else if no_fallback {
            bail!("no display (DISPLAY / WAYLAND_DISPLAY unset); refusing to fall back");
        } else {
            eprintln!("stint: no display (DISPLAY / WAYLAND_DISPLAY unset); falling back to the TUI");
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        if no_fallback {
            bail!("this build has no GUI (built without the `gui` feature)");
        }
        eprintln!("stint: this build has no GUI; falling back to the TUI");
    }
    cmd_tui(home)
}

fn cmd_mcp(home: &Home) -> Result<()> {
    #[cfg(feature = "mcp")]
    {
        // stdout is the protocol channel; anything human goes to stderr.
        eprintln!("stint mcp: serving {} over stdio", home.log_dir.display());
        return mcp::run(home.clone());
    }
    #[cfg(not(feature = "mcp"))]
    {
        let _ = home;
        bail!("this build has no MCP server (built without the `mcp` feature)");
    }
}

/// The terminal fallback. Until the ratatui port lands this execs the Python
/// Textual TUI the same way `stint.sh tui` does, so the fallback is the app the
/// user already knows. Demo mode is passed through via STINT_DEMO.
fn cmd_tui(home: &Home) -> Result<()> {
    let project = if home.demo { home.log_dir.parent().map(|p| p.to_path_buf()).unwrap_or(home.log_dir.clone()) } else { home.log_dir.clone() };
    if !project.join("stintcore").is_dir() {
        bail!(
            "no TUI available: {} has no stintcore/ package (the Python TUI) and the native TUI isn't built yet",
            project.display()
        );
    }
    let mut cmd = std::process::Command::new("uv");
    cmd.args(["run", "--project"]).arg(&project).args(["python", "-m", "stintcore.tui"]);
    if home.demo {
        cmd.env("STINT_DEMO", "1");
    }
    let status = cmd.status().map_err(|e| anyhow::anyhow!("could not run `uv` ({e}); install uv or run `stint.sh tui`"))?;
    if !status.success() {
        bail!("TUI exited with {status}");
    }
    Ok(())
}

fn ym(year: Option<i32>, month: Option<u32>) -> (i32, u32) {
    let today = Local::now().date_naive();
    (year.unwrap_or(today.year()), month.unwrap_or(today.month()))
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Legacy `render_preview` layout.
fn print_preview(items: &[invoice::LineItem], raw_total: f64) {
    let t = invoice::preview_totals(items, raw_total);
    println!();
    println!("{}", "═".repeat(72));
    println!("  Consolidated Invoice Preview");
    println!("{}", "═".repeat(72));
    println!("  {:<22} {:>6}  {:>9}", "Label", "Hrs", "Total");
    println!("{}", "─".repeat(72));
    for it in items {
        println!("  {:<22} {:>6.2}  ${:>8.2}", it.label, it.hours, it.total());
        for line in wrap(&it.narrative, 66) {
            println!("    {line}");
        }
        println!();
    }
    println!("{}", "─".repeat(72));
    println!("  Raw CSV total : {:.4}h  (${:.2})", t.raw_hours, t.raw_amount);
    println!("  Invoice total : {:.2}h  (${:.2})", t.invoice_hours, t.invoice_amount);
    let delta = if t.delta >= 0.0 { format!("+${:.2}", t.delta) } else { format!("-${:.2}", t.delta.abs()) };
    println!("  Delta         : {delta}");
    println!("{}", "═".repeat(72));
}

fn cmd_consolidate(home: &Home, year: Option<i32>, month: Option<u32>, json: bool) -> Result<()> {
    let (year, month) = ym(year, month);
    let (items, raw_total) = invoice::line_items_for(home, year, month)?;
    let path = invoice::write_staging(home, &items, year, month, Local::now().date_naive())?;
    if json {
        println!("{}", serde_json::json!({ "items": items, "totals": invoice::preview_totals(&items, raw_total), "staging": path }));
        return Ok(());
    }
    print_preview(&items, raw_total);
    println!();
    println!("  Staging file  : {}", path.strip_prefix(&home.log_dir).unwrap_or(&path).display());
    println!("  Edit it, then run: stint invoice {year} {month:02} --html");
    println!();
    Ok(())
}

fn cmd_invoice(home: &Home, year: Option<i32>, month: Option<u32>, html: bool, client: &str, json: bool) -> Result<()> {
    let (year, month) = ym(year, month);
    let entries = invoice::load_entries(home, year, month)?;
    let (auto_items, raw_total) = invoice::line_items_for(home, year, month)?;
    let staged = invoice::read_staging(home, year, month)?;
    let items = match &staged {
        Some(s) => s.clone(),
        None => auto_items,
    };
    if !html {
        if json {
            println!("{}", serde_json::json!({ "items": items, "staged": staged.is_some(), "totals": invoice::preview_totals(&items, raw_total) }));
        } else {
            if staged.is_some() {
                println!("Using staged items from temp/staging-{year}-{month:02}.txt");
            }
            print_preview(&items, raw_total);
        }
        return Ok(());
    }
    let inv_num = invoice::next_invoice_number(home, client)?;
    let out = invoice::build_html(home, &items, &entries, year, month, inv_num, client)?;
    let path = invoice::out_path_for(home, year, month);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, out)?;
    invoice::commit_invoice_number(home, inv_num, client)?;
    if json {
        println!("{}", serde_json::json!({ "invoice_number": inv_num, "path": path, "items": items.len() }));
    } else {
        if staged.is_some() {
            println!("Using staged items from temp/staging-{year}-{month:02}.txt");
        }
        println!("✓ Invoice #{inv_num} written: {}", path.display());
        println!("  Open it in a browser and Print to PDF.");
    }
    Ok(())
}

fn cmd_parity(home: &Home, p: ParityArgs) -> Result<()> {
    match p.probe {
        Probe::Items { year, month } => {
            let (items, raw) = invoice::line_items_for(home, year, month)?;
            println!("{}", serde_json::json!({ "items": items, "raw_total": raw }));
        }
        Probe::Staging { year, month } => {
            let (items, _) = invoice::line_items_for(home, year, month)?;
            let dir = tempfile::tempdir()?;
            let scratch = Home::new(dir.path());
            let path = invoice::write_staging(&scratch, &items, year, month, chrono::NaiveDate::from_ymd_opt(2026, 7, 10).unwrap())?;
            print!("{}", std::fs::read_to_string(path)?);
        }
        Probe::Html { year, month, inv_num } => {
            let entries = invoice::load_entries(home, year, month)?;
            let (items, _) = invoice::line_items_for(home, year, month)?;
            print!("{}", invoice::build_html(home, &items, &entries, year, month, inv_num, &home.default_client)?);
        }
        Probe::Row { entry_date, start, end, category, description } => {
            let r = engine::stop(&entry_date, &start, &category, &description, Some(&end), Some(&[]))?;
            println!("{}", r.csv_row());
        }
        Probe::Hrs { start, end } => println!("{}", engine::duration_hrs(&start, &end)?),
        Probe::Bill { hrs } => println!("{}", engine::billable_for_hrs(&hrs)),
    }
    Ok(())
}
