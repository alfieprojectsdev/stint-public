//! Invoice consolidation, ported from `stintcore/invoice.py` (itself a
//! byte-parity port of the legacy `legacy-consolidate.py`).
//!
//! Byte-parity contract: for the default client this module produces line items,
//! staging text and HTML byte-identical to the Python module for the same CSV
//! (enforced by `tests/test_rust_invoice_parity.py`). To keep that promise the
//! arithmetic stays in `f64` exactly as the legacy float code did:
//!   * `round_quarter` is Python `round(h*4)/4`, which rounds ties to even.
//!   * Line-item hours are formatted with `{:.2}`; sums are sequential.
//!   * Narrative selection sorts by *code point* length (Python `len(str)`),
//!     not byte length.
//!   * `build_html` reproduces one quirk of the Python implementation: the
//!     JS row array is inserted through `re.sub`, whose replacement template
//!     re-processes `\\` and `\n` escapes. See [`py_template_unescape`].
//!
//! Historical short entries are summed AS STORED: the quarter-hour rule is a
//! stop-time rule and is never re-applied here, so regenerating a past month
//! never silently changes its total.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use chrono::{Datelike, NaiveDate};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::{self, Client, Home};

/// Float, to preserve byte-parity with the legacy float arithmetic.
pub const RATE: f64 = 16.0;

#[derive(Debug, thiserror::Error)]
pub enum InvoiceError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Csv(#[from] csv::Error),
    #[error("No log file found: {0}")]
    NoLog(PathBuf),
    #[error("No invoice template at {0} (copy samples/temp/invoice-dynamic.html there)")]
    NoTemplate(PathBuf),
    #[error("unknown client {0:?}")]
    UnknownClient(String),
    #[error("Client '{key}' ({name}) is not HTML-capable — its invoices ({currency}, withholding tax) are hand-built. Reserve the number with next_invoice_number and build the HTML manually.")]
    NotHtmlCapable { key: String, name: String, currency: String },
    #[error("bad invoice counter in {0}")]
    BadCounter(PathBuf),
}

pub type Result<T> = std::result::Result<T, InvoiceError>;

/// One consolidated invoice row. `hours` is already quarter-rounded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineItem {
    pub label: String,
    pub narrative: String,
    pub hours: f64,
    #[serde(default = "default_rate")]
    pub rate: f64,
}

fn default_rate() -> f64 {
    RATE
}

impl LineItem {
    pub fn total(&self) -> f64 {
        self.hours * self.rate
    }
}

/// A CSV row as the consolidator sees it (legacy `load_entries` dict).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawEntry {
    pub date: String,
    pub start_time: String,
    pub end_time: String,
    pub hrs: f64,
    pub category: String,
    pub description: String,
}

// ── Load ─────────────────────────────────────────────────────────────────────

/// Legacy `load_entries`: `csv.DictReader` semantics (header row, positional
/// columns, fields stripped, unparseable hours -> 0.0). Unlike
/// `store::read_month` this does NOT rejoin extra columns into the
/// description, because DictReader never did.
pub fn load_entries(home: &Home, year: i32, month: u32) -> Result<Vec<RawEntry>> {
    let path = home.csv_path(year, month);
    if !path.exists() {
        return Err(InvoiceError::NoLog(path));
    }
    let mut reader = csv::ReaderBuilder::new().has_headers(true).flexible(true).from_path(&path)?;
    let headers = reader.headers()?.clone();
    let idx = |name: &str| headers.iter().position(|h| h == name);
    let (i_date, i_start, i_end, i_hrs, i_cat, i_desc) = (
        idx("date"),
        idx("start_time"),
        idx("end_time"),
        idx("duration_hrs"),
        idx("category"),
        idx("description"),
    );
    let field = |rec: &csv::StringRecord, i: Option<usize>| -> String {
        i.and_then(|i| rec.get(i)).unwrap_or("").trim().to_string()
    };
    let mut entries = Vec::new();
    for rec in reader.records() {
        let rec = match rec {
            Ok(r) => r,
            Err(_) => continue,
        };
        let hrs = i_hrs
            .and_then(|i| rec.get(i))
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(0.0);
        entries.push(RawEntry {
            date: field(&rec, i_date),
            start_time: field(&rec, i_start),
            end_time: field(&rec, i_end),
            hrs,
            category: field(&rec, i_cat),
            description: field(&rec, i_desc),
        });
    }
    Ok(entries)
}

// ── Grouping (verbatim port) ─────────────────────────────────────────────────

fn regexes() -> &'static (Regex, Regex, Regex, Regex) {
    use std::sync::OnceLock;
    static RE: OnceLock<(Regex, Regex, Regex, Regex)> = OnceLock::new();
    RE.get_or_init(|| {
        (
            Regex::new(r"(?i)PR\s+#(\d+)").unwrap(),
            Regex::new(r"(?i)issue\s+#(\d+)").unwrap(),
            Regex::new(r"\bT(\d+)\b").unwrap(),
            Regex::new(r"\s*[—–]\s*").unwrap(),
        )
    })
}

/// Leftmost ticket reference in a description: `PR #123`, `issue #123`, `T123`.
pub fn extract_group_key(description: &str) -> Option<String> {
    let (re_pr, re_issue, re_t, _) = regexes();
    let mut matches: Vec<(usize, String)> = Vec::new();
    for m in re_pr.captures_iter(description) {
        matches.push((m.get(0).unwrap().start(), format!("PR #{}", &m[1])));
    }
    for m in re_issue.captures_iter(description) {
        matches.push((m.get(0).unwrap().start(), format!("issue #{}", &m[1])));
    }
    for m in re_t.captures_iter(description) {
        matches.push((m.get(0).unwrap().start(), format!("T{}", &m[1])));
    }
    // Python `min(matches, key=start)` returns the first of equal minima.
    matches.into_iter().min_by_key(|(s, _)| *s).map(|(_, k)| k)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GroupKey {
    Ticket(String),
    Category(String),
}

/// Ticket groups first (insertion order), then category groups.
pub fn group_entries(entries: &[RawEntry]) -> Vec<(GroupKey, Vec<RawEntry>)> {
    let mut tickets: Vec<(GroupKey, Vec<RawEntry>)> = Vec::new();
    let mut cats: Vec<(GroupKey, Vec<RawEntry>)> = Vec::new();
    for e in entries {
        let (bucket, key) = match extract_group_key(&e.description) {
            Some(k) => (&mut tickets, GroupKey::Ticket(k)),
            None => (&mut cats, GroupKey::Category(e.category.clone())),
        };
        match bucket.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(e.clone()),
            None => bucket.push((key, vec![e.clone()])),
        }
    }
    tickets.extend(cats);
    tickets
}

/// CPython >= 3.12 `sum()` over floats: Neumaier compensated summation
/// (`builtin_sum_impl`, the `cs_add` path). A naive left fold differs in the
/// last ulp often enough to flip a quarter-hour rounding, so this is ported
/// verbatim rather than approximated.
pub fn py_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut total = 0.0f64;
    let mut c = 0.0f64;
    for x in values {
        let t = total + x;
        if total.abs() >= x.abs() {
            c += (total - t) + x;
        } else {
            c += (x - t) + total;
        }
        total = t;
    }
    if c != 0.0 && c.is_finite() {
        total += c;
    }
    total
}

/// Python `round(h * 4) / 4`: ties to even.
pub fn round_quarter(hrs: f64) -> f64 {
    (hrs * 4.0).round_ties_even() / 4.0
}

fn em_dash_phrase(description: &str) -> Option<String> {
    let (_, _, _, re_dash) = regexes();
    let mut parts = re_dash.splitn(description, 2);
    let _head = parts.next()?;
    parts.next().map(|p| p.trim().to_string())
}

/// Python `str.title()` for the category fallback label (ASCII is all that
/// ever reaches here: categories are lowercase tokens).
fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_alpha = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_alpha {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_alpha = true;
        } else {
            out.push(c);
            prev_alpha = false;
        }
    }
    out
}

pub fn build_line_item(key: &GroupKey, entries: &[RawEntry]) -> LineItem {
    let label = match key {
        GroupKey::Ticket(k) => k.clone(),
        GroupKey::Category(c) => {
            if config::is_category(c) {
                config::category_label(c).to_string()
            } else {
                py_title(c)
            }
        }
    };

    // Stable sort, longest description first, by code-point length (Python len).
    let mut sorted: Vec<&RawEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| b.description.chars().count().cmp(&a.description.chars().count()));
    let mut narrative = sorted[0].description.clone();

    let mut phrases: Vec<String> = Vec::new();
    for e in &sorted[1..] {
        if let Some(p) = em_dash_phrase(&e.description) {
            if !p.is_empty() && !phrases.contains(&p) && !narrative.contains(&p) {
                phrases.push(p);
            }
        }
        if phrases.len() >= 2 {
            break;
        }
    }
    if !phrases.is_empty() {
        narrative = format!("{} — {}", narrative, phrases.join(", "));
    }

    let hours = round_quarter(py_sum(entries.iter().map(|e| e.hrs)));
    LineItem { label, narrative, hours, rate: RATE }
}

/// Grouped line items + the raw (unrounded) total hours for a month.
pub fn line_items_for(home: &Home, year: i32, month: u32) -> Result<(Vec<LineItem>, f64)> {
    let entries = load_entries(home, year, month)?;
    let items = group_entries(&entries).iter().map(|(k, es)| build_line_item(k, es)).collect();
    let raw_total = py_sum(entries.iter().map(|e| e.hrs));
    Ok((items, raw_total))
}

// ── Staging (verbatim port) ──────────────────────────────────────────────────

pub fn staging_path(home: &Home, year: i32, month: u32) -> PathBuf {
    home.temp_dir().join(format!("staging-{year}-{month:02}.txt"))
}

pub fn write_staging(home: &Home, items: &[LineItem], year: i32, month: u32, today: NaiveDate) -> Result<PathBuf> {
    let path = staging_path(home, year, month);
    let mut lines = vec![
        format!("# stint.sh invoice staging — {year}-{month:02}"),
        format!("# Generated: {}", today.format("%Y-%m-%d")),
        "# Edit label, hours, and narrative. Delete rows to exclude.".to_string(),
        "# Rate is fixed at $16.00/hr.".to_string(),
        "#".to_string(),
        "# label | hours | narrative".to_string(),
        "#".to_string(),
    ];
    for it in items {
        lines.push(format!("{} | {:.2} | {}", it.label, it.hours, it.narrative));
    }
    fs::create_dir_all(path.parent().unwrap())?;
    let mut f = fs::File::create(&path)?;
    f.write_all((lines.join("\n") + "\n").as_bytes())?;
    Ok(path)
}

/// Staged items, or None when there's no staging file or it has no rows.
pub fn read_staging(home: &Home, year: i32, month: u32) -> Result<Option<Vec<LineItem>>> {
    let path = staging_path(home, year, month);
    if !path.exists() {
        return Ok(None);
    }
    let mut items = Vec::new();
    for raw in fs::read_to_string(&path)?.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, " | ").collect();
        if parts.len() < 3 {
            continue;
        }
        let Ok(hours) = parts[1].trim().parse::<f64>() else { continue };
        items.push(LineItem { label: parts[0].trim().into(), narrative: parts[2].trim().into(), hours, rate: RATE });
    }
    Ok(if items.is_empty() { None } else { Some(items) })
}

// ── Counter (per-client) ─────────────────────────────────────────────────────

fn client_or_err<'a>(home: &'a Home, key: &str) -> Result<&'a Client> {
    home.client(key).ok_or_else(|| InvoiceError::UnknownClient(key.to_string()))
}

/// The number the next invoice would get, without touching the counter file
/// (for display). `next_invoice_number` is the seeding/reading form.
pub fn peek_invoice_number(home: &Home, client_key: &str) -> Result<u32> {
    let client = client_or_err(home, client_key)?;
    let path = home.counter_path(client);
    if !path.exists() {
        return Ok(client.counter_seed);
    }
    fs::read_to_string(&path)?.trim().parse().map_err(|_| InvoiceError::BadCounter(path))
}

/// Next invoice number for a client; seeds the counter file on first use.
pub fn next_invoice_number(home: &Home, client_key: &str) -> Result<u32> {
    let client = client_or_err(home, client_key)?;
    let path = home.counter_path(client);
    if !path.exists() {
        fs::write(&path, format!("{}\n", client.counter_seed))?;
        return Ok(client.counter_seed);
    }
    fs::read_to_string(&path)?.trim().parse().map_err(|_| InvoiceError::BadCounter(path))
}

/// Record that `current` has been used: counter := current + 1 (atomic).
pub fn commit_invoice_number(home: &Home, current: u32, client_key: &str) -> Result<()> {
    let client = client_or_err(home, client_key)?;
    let path = home.counter_path(client);
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let res = fs::write(&tmp, format!("{}\n", current + 1)).and_then(|_| fs::rename(&tmp, &path));
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(res?)
}

// ── HTML render (verbatim port + client header) ──────────────────────────────

pub fn template_path(home: &Home) -> PathBuf {
    home.temp_dir().join("invoice-dynamic.html")
}

pub fn out_path_for(home: &Home, year: i32, month: u32) -> PathBuf {
    home.temp_dir().join(format!("invoice-{year}-{month:02}.html"))
}

pub fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let first_next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    first_next.unwrap().pred_opt().unwrap()
}

/// Python `strftime("%-d %B %Y")`.
pub fn fmt_date(d: NaiveDate) -> String {
    format!("{} {} {}", d.day(), d.format("%B"), d.year())
}

fn html_escape(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn js_escape(t: &str) -> String {
    t.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "")
}

/// Python `re.sub` treats its replacement argument as a template: `\\`
/// becomes one backslash and `\n` a newline, while `\"` is left alone. The
/// Python port passes the JS-escaped row array through that template, so we
/// apply the same transformation to stay byte-identical (this is faithful to
/// a latent bug: a narrative with a literal backslash or newline would break
/// the generated JS in both implementations).
fn py_template_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.peek() {
                Some('\\') => {
                    it.next();
                    out.push('\\');
                }
                Some('n') => {
                    it.next();
                    out.push('\n');
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn sub_all(template: &str, pattern: &str, text: &str) -> String {
    let re = Regex::new(pattern).unwrap();
    re.replace_all(template, |caps: &regex::Captures| format!("{}{}{}", &caps[1], text, &caps[2])).into_owned()
}

/// Pure HTML string builder: no counter side effects, no output file.
pub fn build_html(
    home: &Home,
    items: &[LineItem],
    entries: &[RawEntry],
    year: i32,
    month: u32,
    inv_num: u32,
    client_key: &str,
) -> Result<String> {
    let client = client_or_err(home, client_key)?;
    if !client.html_capable {
        return Err(InvoiceError::NotHtmlCapable {
            key: client.key.clone(),
            name: client.display_name.clone(),
            currency: client.currency.clone(),
        });
    }

    let issue_date = last_day_of_month(year, month);
    let due_date = issue_date + chrono::Duration::days(15);

    let real_dates: Vec<&str> = entries.iter().map(|e| e.date.as_str()).filter(|d| !d.is_empty() && *d != "manual").collect();
    let (min_d, max_d) = if real_dates.is_empty() {
        (NaiveDate::from_ymd_opt(year, month, 1).unwrap(), issue_date)
    } else {
        // Python compares the ISO strings, then parses; same result for
        // well-formed dates.
        let lo = real_dates.iter().min().unwrap();
        let hi = real_dates.iter().max().unwrap();
        (
            NaiveDate::parse_from_str(lo, "%Y-%m-%d").unwrap_or(issue_date),
            NaiveDate::parse_from_str(hi, "%Y-%m-%d").unwrap_or(issue_date),
        )
    };
    let billing_period = format!("Billing Period: {} - {}", fmt_date(min_d), fmt_date(max_d));

    let tpath = template_path(home);
    let template = fs::read_to_string(&tpath).map_err(|_| InvoiceError::NoTemplate(tpath.clone()))?;

    let mut js_rows = Vec::new();
    for it in items {
        let desc_html = format!("<strong>{}:</strong> {}", html_escape(&it.label), html_escape(&it.narrative));
        js_rows.push(format!(
            "            {{ desc: \"{}\", hours: {:.2}, rate: {:.2} }}",
            js_escape(&desc_html),
            it.hours,
            it.rate
        ));
    }
    let js_array = format!("[\n{}\n        ]", js_rows.join(",\n"));
    let js_repl = py_template_unescape(&format!("const defaultItems = {js_array};"));

    let re_items = Regex::new(r"(?s)const defaultItems = \[.*?\];").unwrap();
    let mut t = re_items.replace_all(&template, regex::NoExpand(&js_repl)).into_owned();
    let re_key = Regex::new(r"const STORAGE_KEY = '[^']*';").unwrap();
    t = re_key.replacen(&t, 1, regex::NoExpand(&format!("const STORAGE_KEY = 'stint_invoice_{year}_{month:02}';"))).into_owned();
    t = sub_all(&t, r#"(<span id="invoice-number"[^>]*>)[^<]*(</span>)"#, &inv_num.to_string());
    t = sub_all(&t, r#"(<span id="issue-date"[^>]*>)[^<]*(</span>)"#, &fmt_date(issue_date));
    t = sub_all(&t, r#"(<span id="due-date"[^>]*>)[^<]*(</span>)"#, &fmt_date(due_date));
    let project_title: &str = if client.project_title.is_empty() { "Software Development" } else { &client.project_title };
    t = sub_all(&t, r#"(<h3 id="project-title"[^>]*>)[^<]*(</h3>)"#, &html_escape(project_title));
    t = sub_all(&t, r#"(<p id="billing-period"[^>]*>)[^<]*(</p>)"#, &billing_period);
    if !client.legal_name.is_empty() {
        t = sub_all(&t, r#"(<p id="client-name"[^>]*>)[^<]*(</p>)"#, &html_escape(&client.legal_name));
    }
    for (i, line) in client.address.iter().take(3).enumerate() {
        let pat = format!(r#"(<p id="client-addr-{}"[^>]*>)[^<]*(</p>)"#, i + 1);
        t = sub_all(&t, &pat, &html_escape(line));
    }
    Ok(t)
}

/// Summary figures for a preview: rounded invoice total vs raw CSV total.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PreviewTotals {
    pub raw_hours: f64,
    pub raw_amount: f64,
    pub invoice_hours: f64,
    pub invoice_amount: f64,
    pub delta: f64,
}

pub fn preview_totals(items: &[LineItem], raw_total: f64) -> PreviewTotals {
    let invoice_amount = py_sum(items.iter().map(LineItem::total));
    let invoice_hours = py_sum(items.iter().map(|i| i.hours));
    PreviewTotals {
        raw_hours: raw_total,
        raw_amount: raw_total * RATE,
        invoice_hours,
        invoice_amount,
        delta: invoice_amount - raw_total * RATE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_key_is_leftmost_reference() {
        assert_eq!(extract_group_key("PR #91 skim for T23"), Some("PR #91".into()));
        assert_eq!(extract_group_key("T23 retry layer, see pr #4"), Some("T23".into()));
        assert_eq!(extract_group_key("fix Issue #7 then T1"), Some("issue #7".into()));
        assert_eq!(extract_group_key("plain work"), None);
        assert_eq!(extract_group_key("T23abc"), None); // \b
    }

    #[test]
    fn py_sum_is_compensated() {
        // Naive fold gives 17.369999999999997; CPython 3.12+ sum() gives 17.37.
        let xs = [2.20, 0.33, 2.60, 1.50, 1.22, 0.33, 2.43, 0.75, 2.31, 1.29, 1.58, 0.33, 0.50]; // samples/stint-2026-06.csv hours
        let naive = xs.iter().fold(0.0, |a, b| a + b);
        let py = py_sum(xs.iter().copied());
        assert_ne!(naive, py);
        assert_eq!(py, 17.37);
    }

    #[test]
    fn round_quarter_ties_to_even_like_python() {
        assert_eq!(round_quarter(0.125), 0.0); // 0.5 -> 0 (even)
        assert_eq!(round_quarter(0.375), 0.5); // 1.5 -> 2
        assert_eq!(round_quarter(0.625), 0.5); // 2.5 -> 2
        assert_eq!(round_quarter(2.20), 2.25);
        assert_eq!(round_quarter(0.33), 0.25);
    }

    #[test]
    fn py_template_unescape_matches_re_sub() {
        assert_eq!(py_template_unescape(r#"a \" b"#), r#"a \" b"#);
        assert_eq!(py_template_unescape(r"a \\ b"), r"a \ b");
        assert_eq!(py_template_unescape(r"a \n b"), "a \n b");
    }

    #[test]
    fn fmt_date_has_no_zero_padding() {
        assert_eq!(fmt_date(NaiveDate::from_ymd_opt(2026, 4, 5).unwrap()), "5 April 2026");
        assert_eq!(last_day_of_month(2026, 2), NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
        assert_eq!(last_day_of_month(2026, 12), NaiveDate::from_ymd_opt(2026, 12, 31).unwrap());
    }

    #[test]
    fn narrative_merges_em_dash_phrases() {
        let mk = |d: &str, h: f64| RawEntry { date: "2026-06-01".into(), start_time: "manual".into(), end_time: "manual".into(), hrs: h, category: "pr".into(), description: d.into() };
        let es = vec![mk("PR #91 skim — LGTM pending CI", 0.33), mk("PR #91 review — backoff edge cases, added chaos test", 2.60), mk("PR #91 — LGTM pending CI", 0.1)];
        let item = build_line_item(&GroupKey::Ticket("PR #91".into()), &es);
        assert_eq!(item.label, "PR #91");
        assert_eq!(item.narrative, "PR #91 review — backoff edge cases, added chaos test — LGTM pending CI");
        assert_eq!(item.hours, 3.0);
    }
}
