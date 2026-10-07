//! `stint mcp`: a stdio MCP server exposing the ledger operations as typed
//! tools, so a Claude Code session drives stint with structured calls instead
//! of parsing CLI output.
//!
//! Every tool is a thin wrapper over the same `stint-core` calls the CLI and
//! GUI use, so the validation (category delimiters, hours parsing, the
//! quarter-hour rule) and the on-disk format are identical. Results are the
//! `--json` shapes of the matching CLI verb, as a JSON text block.
//!
//! Register once with Claude Code:
//!
//! ```text
//! claude mcp add stint -- stint mcp
//! ```

use chrono::{Datelike, Local};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock as Content, ErrorData as McpError, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_router, ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use stint_core::{config, invoice, store, Home};

#[derive(Clone)]
pub struct StintServer {
    home: Home,
}

fn ok(value: Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(value.to_string())]))
}

/// A failed operation is a tool *result* with `isError`, not a protocol
/// error, so the model sees the message and can recover (e.g. pick an id).
fn fail(msg: impl std::fmt::Display) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![Content::text(msg.to_string())]))
}

/// Resolve year/month defaults and reject a month outside 1..=12 up front:
/// downstream `stint-YYYY-MM.csv` lookups would silently read nothing, and
/// the invoice date arithmetic asserts on an impossible month.
fn ym(year: Option<i32>, month: Option<u32>) -> Result<(i32, u32), String> {
    let today = Local::now().date_naive();
    let (y, m) = (year.unwrap_or(today.year()), month.unwrap_or(today.month()));
    if !(1..=12).contains(&m) {
        return Err(format!("month must be 1-12 (got {m})"));
    }
    if !(2000..=2100).contains(&y) {
        return Err(format!("year must be four digits (got {y})"));
    }
    Ok((y, m))
}

#[derive(Deserialize, JsonSchema)]
pub struct StartTimer {
    /// What the work is. Ticket refs (`T123`, `PR #45`, `issue #7`) group entries on the invoice.
    pub description: String,
    /// One of: pr, async, standup, devops, research, dev, admin, docs, planning. Default dev.
    pub category: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct StopTimer {
    /// Timer id from list_running. Required only when several timers are running.
    pub id: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MonthArgs {
    /// Four-digit year; defaults to the current year.
    pub year: Option<i32>,
    /// 1-12; defaults to the current month.
    pub month: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ConsolidateArgs {
    /// Four-digit year; defaults to the current year.
    pub year: Option<i32>,
    /// 1-12; defaults to the current month.
    pub month: Option<u32>,
    /// Regenerate temp/staging-YYYY-MM.txt even if one exists (discarding hand edits). Default false: an existing staging file is returned as-is.
    pub overwrite: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AddEntry {
    pub description: String,
    /// One of: pr, async, standup, devops, research, dev, admin, docs, planning.
    pub category: String,
    /// Decimal hours as typed on the invoice, e.g. "1.5". Manual entries are exempt from the quarter-hour rule.
    pub hours: String,
    /// YYYY-MM-DD; defaults to today.
    pub date: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RenderInvoice {
    pub year: Option<i32>,
    pub month: Option<u32>,
    /// Client key from clients.json (default: its default_client). A hand-built client only reserves a number.
    pub client: Option<String>,
}

#[tool_router]
impl StintServer {
    pub fn new(home: Home) -> Self {
        StintServer { home }
    }

    #[tool(description = "Start a timer. Returns the timer id and start time. Same rules as `stint start`.")]
    fn start_timer(&self, Parameters(p): Parameters<StartTimer>) -> Result<CallToolResult, McpError> {
        let category = p.category.unwrap_or_else(|| config::DEFAULT_CATEGORY.to_string());
        if p.description.trim().is_empty() {
            return fail("description is required");
        }
        match store::start_timer(&self.home, p.description.trim(), &category) {
            Ok(t) => {
                let mut v = json!(t);
                if !config::is_category(&category) {
                    v["note"] = format!("{category:?} is not a known category ({})", config::CATEGORIES.join(", ")).into();
                }
                ok(v)
            }
            Err(e) => fail(e),
        }
    }

    #[tool(description = "Stop a running timer and log it. Billable hours are the measured time rounded UP to the next quarter hour (0.25h minimum); this is the intended billing convention, not a bug. If several timers run, pass id.")]
    fn stop_timer(&self, Parameters(p): Parameters<StopTimer>) -> Result<CallToolResult, McpError> {
        let timer = match p.id {
            Some(id) => match store::find_timer(&self.home, id) {
                Ok(t) => t,
                Err(e) => return fail(e),
            },
            None => {
                let timers = match store::read_timers(&self.home) {
                    Ok(t) => t,
                    Err(e) => return fail(e),
                };
                match timers.len() {
                    0 => return fail("no timers running"),
                    1 => timers.into_iter().next().unwrap(),
                    _ => {
                        let ids: Vec<u32> = timers.iter().map(|t| t.timer_id).collect();
                        return fail(format!("multiple timers running; pass id (running: {ids:?})"));
                    }
                }
            }
        };
        match store::stop_timer(&self.home, &timer, None) {
            Ok(r) => {
                let mut v = json!(r);
                v["timer_id"] = timer.timer_id.into();
                v["billable"] = r.billable().into();
                v["csv_row"] = r.csv_row().into();
                ok(v)
            }
            Err(e) => fail(e),
        }
    }

    #[tool(description = "List running timers with elapsed time and the indicative amount so far.")]
    fn list_running(&self) -> Result<CallToolResult, McpError> {
        let now = Local::now().naive_local();
        match store::read_timers(&self.home) {
            Ok(timers) => ok(Value::Array(
                timers
                    .iter()
                    .map(|t| {
                        let mut v = json!(t);
                        v["elapsed_seconds"] = t.elapsed_seconds(now).into();
                        v["elapsed_hms"] = t.elapsed_hms(now).into();
                        v["live_amount"] = format!("{:.2}", t.live_amount(now)).into();
                        v
                    })
                    .collect(),
            )),
            Err(e) => fail(e),
        }
    }

    #[tool(description = "Logged entries for a month, as stored, plus totals.")]
    fn log_month(&self, Parameters(p): Parameters<MonthArgs>) -> Result<CallToolResult, McpError> {
        let (y, m) = match ym(p.year, p.month) {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        match store::read_month(&self.home, y, m) {
            Ok(entries) => {
                let t = store::sum(&entries);
                ok(json!({ "year": y, "month": m, "entries": entries, "totals": t }))
            }
            Err(e) => fail(e),
        }
    }

    #[tool(description = "Add a manual (backdated) entry with explicit hours. Hours are written verbatim; no quarter-hour rounding.")]
    fn add_entry(&self, Parameters(p): Parameters<AddEntry>) -> Result<CallToolResult, McpError> {
        let hours = p.hours.trim();
        let parsed: Result<rust_decimal::Decimal, _> = hours.parse();
        match parsed {
            Ok(h) if !h.is_sign_negative() => {}
            _ => return fail(format!("hours must be a non-negative decimal like 1.5 (got {hours:?})")),
        }
        if p.description.trim().is_empty() {
            return fail("description is required");
        }
        let category = p.category.trim();
        match store::add_entry(&self.home, p.description.trim(), category, hours, p.date.as_deref()) {
            Ok(e) => {
                let mut v = json!(e);
                if !config::is_category(category) {
                    v["note"] = format!("{category:?} is not a known category ({})", config::CATEGORIES.join(", ")).into();
                }
                ok(v)
            }
            Err(e) => fail(e),
        }
    }

    #[tool(description = "Today / this week (Mon-Sun) / this month totals of logged entries.")]
    fn summary(&self) -> Result<CallToolResult, McpError> {
        match store::summary(&self.home, Local::now().date_naive()) {
            Ok(s) => ok(json!(s)),
            Err(e) => fail(e),
        }
    }

    #[tool(description = "Group a month's entries into invoice line items (by ticket ref, then category), quarter-rounded, and write temp/staging-YYYY-MM.txt for editing. If a staging file already exists (someone edited the lines), it is returned unchanged unless overwrite=true. Returns the items and raw-vs-invoice totals.")]
    fn consolidate_preview(&self, Parameters(p): Parameters<ConsolidateArgs>) -> Result<CallToolResult, McpError> {
        let (y, m) = match ym(p.year, p.month) {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        let (auto_items, raw) = match invoice::line_items_for(&self.home, y, m) {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        let path = invoice::staging_path(&self.home, y, m);
        if !p.overwrite.unwrap_or(false) {
            match invoice::read_staging(&self.home, y, m) {
                Ok(Some(staged)) => {
                    return ok(json!({ "items": staged, "staged": true, "totals": invoice::preview_totals(&staged, raw), "staging": path,
                        "note": "existing staging file returned unchanged; pass overwrite=true to regenerate from the ledger" }));
                }
                Ok(None) => {}
                Err(e) => return fail(e),
            }
        }
        match invoice::write_staging(&self.home, &auto_items, y, m, Local::now().date_naive()) {
            Ok(path) => ok(json!({ "items": auto_items, "staged": false, "totals": invoice::preview_totals(&auto_items, raw), "staging": path })),
            Err(e) => fail(e),
        }
    }

    #[tool(description = "Render temp/invoice-YYYY-MM.html from the staged line items (or the auto grouping) and advance the client's invoice counter. For a hand-built client (html_capable=false) this only reserves the next number.")]
    fn render_invoice(&self, Parameters(p): Parameters<RenderInvoice>) -> Result<CallToolResult, McpError> {
        let (y, m) = match ym(p.year, p.month) {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        let client = p.client.unwrap_or_else(|| self.home.default_client.clone());
        let Some(c) = self.home.client(&client).cloned() else {
            let keys: Vec<&str> = self.home.clients.iter().map(|c| c.key.as_str()).collect();
            return fail(format!("unknown client {client:?} (known: {keys:?})"));
        };
        let n = match invoice::next_invoice_number(&self.home, &client) {
            Ok(n) => n,
            Err(e) => return fail(e),
        };
        if !c.html_capable {
            return match invoice::commit_invoice_number(&self.home, n, &client) {
                Ok(()) => ok(json!({ "client": client, "reserved_invoice_number": n, "html": null,
                    "note": format!("{} invoices ({}) are hand-built; only the number was reserved", c.display_name, c.currency) })),
                Err(e) => fail(e),
            };
        }
        let entries = match invoice::load_entries(&self.home, y, m) {
            Ok(v) => v,
            Err(e) => return fail(e),
        };
        let items = match invoice::read_staging(&self.home, y, m) {
            Ok(Some(s)) => s,
            Ok(None) => match invoice::line_items_for(&self.home, y, m) {
                Ok((i, _)) => i,
                Err(e) => return fail(e),
            },
            Err(e) => return fail(e),
        };
        let html = match invoice::build_html(&self.home, &items, &entries, y, m, n, &client) {
            Ok(h) => h,
            Err(e) => return fail(e),
        };
        let path = invoice::out_path_for(&self.home, y, m);
        if let Err(e) = std::fs::create_dir_all(path.parent().unwrap()).and_then(|_| std::fs::write(&path, html)) {
            return fail(format!("write failed: {e}"));
        }
        if let Err(e) = invoice::commit_invoice_number(&self.home, n, &client) {
            return fail(format!("invoice written but counter not advanced: {e}"));
        }
        ok(json!({ "client": client, "invoice_number": n, "path": path, "items": items.len() }))
    }

    #[tool(description = "The ledger folder this server reads and writes, and whether it is the demo dataset.")]
    fn ledger_info(&self) -> Result<CallToolResult, McpError> {
        ok(json!({ "log_dir": self.home.log_dir, "demo": self.home.demo, "months": store::available_months(&self.home),
            "default_client": self.home.default_client, "clients": self.home.clients.iter().map(|c| json!({"key": c.key, "display_name": c.display_name, "html_capable": c.html_capable})).collect::<Vec<_>>() }))
    }
}

#[rmcp::tool_handler]
impl ServerHandler for StintServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("stint", env!("CARGO_PKG_VERSION"));
        info.with_instructions(
            "stint time tracker + invoices. Start/stop timers, add manual entries, read the log and totals, \
             consolidate a month into invoice lines and render the HTML. Billable time is measured time rounded \
             UP to the next quarter hour (intended convention). All writes go to the same CSV/.timer files the CLI \
             and GUI use; the GUI refreshes within a second.",
        )
    }
}

/// Serve over stdio until the client disconnects.
pub fn run(home: Home) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        let server = StintServer::new(home).serve(rmcp::transport::stdio()).await?;
        server.waiting().await?;
        Ok::<(), anyhow::Error>(())
    })
}
