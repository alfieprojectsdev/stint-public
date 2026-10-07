//! Read/write layer over the on-disk state (CSVs + `.timer` files).
//!
//! Port of `stintcore/store.py`. The engine computes billing; this module is
//! the I/O around it. `stop_timer` delegates to `engine::stop` and appends its
//! `csv_row` verbatim, so there is no second copy of the billing arithmetic.
//!
//! Every writer here emits `\n` explicitly (never platform newlines), so a
//! ledger shared between Windows, WSL and Linux stays LF-only.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;

use crate::config::{self, Home};
use crate::engine::{self, EngineError, StopResult, TS_FMT};

pub const MANUAL: &str = "manual";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Csv(#[from] csv::Error),
    #[error("row {index} out of range (month has {len} rows)")]
    RowOutOfRange { index: usize, len: usize },
    #[error("bad date {0:?} (expected YYYY-MM-DD)")]
    BadDate(String),
    #[error("no timer #{0} found")]
    NoSuchTimer(u32),
    #[error("category {0:?} may not contain a comma, quote or newline")]
    BadCategory(String),
    #[error("description may not contain a newline")]
    BadDescription,
}

/// The category column is written bare (unquoted) in both the Bash and the
/// engine row format, so a delimiter inside it shifts every later column.
fn check_category(category: &str) -> Result<()> {
    if category.is_empty() || category.contains([',', '"', '\n', '\r']) {
        return Err(StoreError::BadCategory(category.to_string()));
    }
    Ok(())
}

/// `.timer` files are line-based (`key=value`), so a newline in the
/// description would be silently truncated at the next read.
fn check_description(description: &str) -> Result<()> {
    if description.contains(['\n', '\r']) {
        return Err(StoreError::BadDescription);
    }
    Ok(())
}

pub type Result<T> = std::result::Result<T, StoreError>;

// ── Entries (logged CSV rows) ────────────────────────────────────────────────

/// One logged row from a `stint-YYYY-MM.csv` file, fields kept as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub entry_date: String,
    pub start_time: String,
    pub end_time: String,
    pub hrs: String,
    pub category: String,
    pub description: String,
}

impl Entry {
    pub fn is_manual(&self) -> bool {
        self.start_time == MANUAL
    }

    pub fn date(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(&self.entry_date, "%Y-%m-%d").ok()
    }

    /// Hours as a decimal; unparseable -> 0 (a malformed row must not crash a view).
    pub fn hours(&self) -> Decimal {
        self.hrs.trim().parse().unwrap_or_default()
    }

    /// Billable $ for this row: hrs * RATE, truncated to cents (Bash parity).
    pub fn amount(&self) -> Decimal {
        (self.hours() * config::RATE).round_dp_with_strategy(2, RoundingStrategy::ToZero)
    }
}

pub fn ensure_header(path: &Path) -> Result<()> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, format!("{}\n", config::CSV_HEADER))?;
    }
    Ok(())
}

/// Parse a monthly CSV into rows. Missing file -> empty.
pub fn read_month(home: &Home, year: i32, month: u32) -> Result<Vec<Entry>> {
    let path = home.csv_path(year, month);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(&path)?;
    let mut entries = Vec::new();
    for record in reader.records() {
        let row = match record {
            Ok(r) => r,
            Err(_) => continue, // malformed line: skip, don't crash the view
        };
        if row.is_empty() || row.get(0) == Some("date") {
            continue; // header / blank
        }
        if row.len() < 6 {
            continue;
        }
        // Description containing an unquoted comma is rejoined defensively,
        // exactly like the Python reader.
        let description = row.iter().skip(5).collect::<Vec<_>>().join(",");
        entries.push(Entry {
            entry_date: row[0].trim().to_string(),
            start_time: row[1].trim().to_string(),
            end_time: row[2].trim().to_string(),
            hrs: row[3].trim().to_string(),
            category: row[4].trim().to_string(),
            description,
        });
    }
    Ok(entries)
}

/// All (year, month) pairs with a CSV on disk, newest first.
pub fn available_months(home: &Home) -> Vec<(i32, u32)> {
    let mut months = Vec::new();
    if let Ok(dir) = fs::read_dir(&home.log_dir) {
        for entry in dir.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(ym) = config::parse_month_filename(&name) {
                months.push(ym);
            }
        }
    }
    months.sort_unstable();
    months.dedup();
    months.reverse();
    months
}

/// Serialize an Entry in the ledger format: bare fields + an always-quoted
/// description with embedded quotes doubled per RFC 4180. Rows without quotes
/// serialize byte-identically to the Bash output.
pub fn serialize_row(e: &Entry) -> String {
    let desc = e.description.replace('"', "\"\"");
    format!("{},{},{},{},{},\"{}\"", e.entry_date, e.start_time, e.end_time, e.hrs, e.category, desc)
}

/// Rewrite a monthly CSV atomically (temp file in the same dir + rename).
pub fn write_month(home: &Home, year: i32, month: u32, entries: &[Entry]) -> Result<PathBuf> {
    let path = home.csv_path(year, month);
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".stint-{year}-{month:02}.{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut f = fs::File::create(&tmp)?;
        writeln!(f, "{}", config::CSV_HEADER)?;
        for e in entries {
            writeln!(f, "{}", serialize_row(e))?;
        }
        f.sync_all()?;
        fs::rename(&tmp, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map(|_| path)
}

fn split_ym(entry_date: &str) -> Result<(i32, u32)> {
    let d = NaiveDate::parse_from_str(entry_date, "%Y-%m-%d")
        .map_err(|_| StoreError::BadDate(entry_date.to_string()))?;
    Ok((d.year(), d.month()))
}

/// Append a manual entry (Bash `cmd_add`): start/end = `manual`, hours
/// verbatim (quarter-hour rule exempt). Appends like Bash; no rewrite.
pub fn add_entry(home: &Home, description: &str, category: &str, hrs: &str, entry_date: Option<&str>) -> Result<Entry> {
    check_category(category)?;
    let entry_date = match entry_date {
        // Normalise to zero-padded YYYY-MM-DD so a typed `2026-9-2` sorts and
        // filters like every other row.
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map_err(|_| StoreError::BadDate(d.to_string()))?
            .format("%Y-%m-%d")
            .to_string(),
        None => Local::now().format("%Y-%m-%d").to_string(),
    };
    let (year, month) = split_ym(&entry_date)?;
    let entry = Entry {
        entry_date,
        start_time: MANUAL.into(),
        end_time: MANUAL.into(),
        hrs: hrs.to_string(),
        category: category.to_string(),
        description: description.to_string(),
    };
    let path = home.csv_path(year, month);
    ensure_header(&path)?;
    append_row(&path, &serialize_row(&entry))?;
    Ok(entry)
}

/// Append a fully specified entry (GUI "add" with explicit times). Validates
/// like `add_entry`, but keeps whatever start/end the caller supplies.
pub fn insert_entry(home: &Home, entry: Entry) -> Result<Entry> {
    check_category(&entry.category)?;
    let (year, month) = split_ym(&entry.entry_date)?;
    let path = home.csv_path(year, month);
    ensure_header(&path)?;
    append_row(&path, &serialize_row(&entry))?;
    Ok(entry)
}

/// Replace the row at `index` (0-based, `read_month` order). A changed date
/// moves the row to its new month. Both rewrites are atomic.
pub fn update_entry(home: &Home, year: i32, month: u32, index: usize, new: Entry) -> Result<Entry> {
    let mut rows = read_month(home, year, month)?;
    if index >= rows.len() {
        return Err(StoreError::RowOutOfRange { index, len: rows.len() });
    }
    let (ny, nm) = split_ym(&new.entry_date)?;
    if (ny, nm) != (year, month) {
        // Destination first: if the second write fails the row exists in both
        // months (visible, fixable) rather than in neither (silently lost).
        let mut dest = read_month(home, ny, nm)?;
        dest.push(new.clone());
        write_month(home, ny, nm, &dest)?;
        rows.remove(index);
        write_month(home, year, month, &rows)?;
    } else {
        rows[index] = new.clone();
        write_month(home, year, month, &rows)?;
    }
    Ok(new)
}

pub fn delete_entry(home: &Home, year: i32, month: u32, index: usize) -> Result<Entry> {
    let mut rows = read_month(home, year, month)?;
    if index >= rows.len() {
        return Err(StoreError::RowOutOfRange { index, len: rows.len() });
    }
    let removed = rows.remove(index);
    write_month(home, year, month, &rows)?;
    Ok(removed)
}

fn append_row(path: &Path, row: &str) -> Result<()> {
    let mut f = fs::OpenOptions::new().append(true).create(true).open(path)?;
    writeln!(f, "{row}")?;
    Ok(())
}

// ── Totals ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Totals {
    pub hours: Decimal,
    pub amount: Decimal,
}

pub fn sum(entries: &[Entry]) -> Totals {
    Totals {
        hours: entries.iter().map(Entry::hours).sum(),
        amount: entries.iter().map(Entry::amount).sum(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub today: Totals,
    pub week: Totals,
    pub month: Totals,
}

/// Today / current week (Mon-Sun) / current month totals of logged entries.
/// Reads the previous month too so a week straddling a boundary sums right.
pub fn summary(home: &Home, today: NaiveDate) -> Result<Summary> {
    let cur = read_month(home, today.year(), today.month())?;
    let (py, pm) = if today.month() > 1 { (today.year(), today.month() - 1) } else { (today.year() - 1, 12) };
    let mut window = cur.clone();
    window.extend(read_month(home, py, pm)?);

    let week_start = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let week_end = week_start + chrono::Duration::days(6);

    let in_range = |e: &&Entry, lo: NaiveDate, hi: NaiveDate| e.date().is_some_and(|d| d >= lo && d <= hi);
    Ok(Summary {
        today: sum(&window.iter().filter(|e| in_range(e, today, today)).cloned().collect::<Vec<_>>()),
        week: sum(&window.iter().filter(|e| in_range(e, week_start, week_end)).cloned().collect::<Vec<_>>()),
        month: sum(&cur),
    })
}

// ── Running timers (.timer files) ────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningTimer {
    pub timer_id: u32,
    pub entry_date: String,
    /// `YYYY-MM-DD HH:MM:SS`
    pub start: String,
    pub category: String,
    pub description: String,
    pub path: PathBuf,
}

impl RunningTimer {
    pub fn elapsed_seconds(&self, now: NaiveDateTime) -> i64 {
        match engine::parse_ts(&self.start) {
            Ok(s) => (now - s).num_seconds().max(0),
            Err(_) => 0,
        }
    }

    pub fn elapsed_hms(&self, now: NaiveDateTime) -> String {
        let s = self.elapsed_seconds(now);
        format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    }

    /// Indicative $ so far (raw elapsed x rate, no quarter-hour round-up).
    pub fn live_amount(&self, now: NaiveDateTime) -> Decimal {
        let hrs = Decimal::from(self.elapsed_seconds(now)) / Decimal::from(3600);
        (hrs * config::RATE).round_dp_with_strategy(2, RoundingStrategy::ToZero)
    }
}

fn read_timer_file(path: &Path) -> Result<RunningTimer> {
    let text = fs::read_to_string(path)?;
    let mut id = None;
    let mut date = String::new();
    let mut start = String::new();
    let mut category = config::DEFAULT_CATEGORY.to_string();
    let mut description = String::new();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "id" => id = v.trim().parse().ok(),
            "date" => date = v.to_string(),
            "start" => start = v.to_string(),
            "category" => category = v.to_string(),
            "description" => description = v.to_string(),
            _ => {}
        }
    }
    let timer_id = id
        .or_else(|| path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    Ok(RunningTimer { timer_id, entry_date: date, start, category, description, path: path.to_path_buf() })
}

/// All running timers, ordered by id (matches `stint.sh status`).
pub fn read_timers(home: &Home) -> Result<Vec<RunningTimer>> {
    let dir = home.timers_dir();
    let mut timers = Vec::new();
    if !dir.exists() {
        return Ok(timers);
    }
    for entry in fs::read_dir(&dir)?.flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|e| e == "timer") {
            if let Ok(t) = read_timer_file(&p) {
                timers.push(t);
            }
        }
    }
    timers.sort_by_key(|t| t.timer_id);
    Ok(timers)
}

pub fn find_timer(home: &Home, id: u32) -> Result<RunningTimer> {
    let p = home.timers_dir().join(format!("{id}.timer"));
    if !p.exists() {
        return Err(StoreError::NoSuchTimer(id));
    }
    read_timer_file(&p)
}

/// Lowest free id (Bash: `while [[ -f id.timer ]]; do id++`).
fn next_timer_id(home: &Home) -> u32 {
    let dir = home.timers_dir();
    let mut i = 1;
    while dir.join(format!("{i}.timer")).exists() {
        i += 1;
    }
    i
}

/// Create a new running timer file (Bash `cmd_start`). No CSV write yet.
pub fn start_timer(home: &Home, description: &str, category: &str) -> Result<RunningTimer> {
    check_category(category)?;
    check_description(description)?;
    let dir = home.timers_dir();
    fs::create_dir_all(&dir)?;
    let tid = next_timer_id(home);
    let now = engine::format_ts(&Local::now().naive_local());
    let entry_date = now[..10].to_string();
    let path = dir.join(format!("{tid}.timer"));
    fs::write(
        &path,
        format!("id={tid}\ndate={entry_date}\nstart={now}\ncategory={category}\ndescription={description}\n"),
    )?;
    Ok(RunningTimer { timer_id: tid, entry_date, start: now, category: category.into(), description: description.into(), path })
}

/// Stop a running timer: reboot-cap + quarter-hour round-up (via the engine),
/// append the byte-identical CSV row, remove the timer file.
pub fn stop_timer(home: &Home, timer: &RunningTimer, end_time: Option<&str>) -> Result<StopResult> {
    let result = engine::stop(&timer.entry_date, &timer.start, &timer.category, &timer.description, end_time, None)?;
    let (year, month) = split_ym(&result.entry_date)?;
    let path = home.csv_path(year, month);
    ensure_header(&path)?;
    append_row(&path, &result.csv_row())?;
    match fs::remove_file(&timer.path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(result)
}

/// Format a timestamp the way the ledger stores it.
pub fn now_ts() -> String {
    Local::now().format(TS_FMT).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home() -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        (dir, home)
    }

    fn entry(date: &str, hrs: &str, desc: &str) -> Entry {
        Entry { entry_date: date.into(), start_time: MANUAL.into(), end_time: MANUAL.into(), hrs: hrs.into(), category: "dev".into(), description: desc.into() }
    }

    #[test]
    fn add_read_roundtrip_and_quote_escaping() {
        let (_d, home) = temp_home();
        add_entry(&home, "plain", "dev", "1.50", Some("2026-07-08")).unwrap();
        add_entry(&home, "has, commas", "pr", "0.25", Some("2026-07-09")).unwrap();
        add_entry(&home, "say \"hi\" — ok", "docs", "0.5", Some("2026-07-10")).unwrap();
        let rows = read_month(&home, 2026, 7).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].description, "has, commas");
        assert_eq!(rows[2].description, "say \"hi\" — ok");
        assert!(rows[0].is_manual());
        let text = fs::read_to_string(home.csv_path(2026, 7)).unwrap();
        assert!(text.starts_with(config::CSV_HEADER));
        assert!(!text.contains('\r'));
        assert!(text.contains("\"say \"\"hi\"\" — ok\""));
    }

    #[test]
    fn update_moves_between_months_and_delete_rewrites() {
        let (_d, home) = temp_home();
        add_entry(&home, "a", "dev", "1", Some("2026-07-08")).unwrap();
        add_entry(&home, "b", "dev", "2", Some("2026-07-09")).unwrap();
        let moved = update_entry(&home, 2026, 7, 0, entry("2026-08-01", "1", "a moved")).unwrap();
        assert_eq!(moved.description, "a moved");
        assert_eq!(read_month(&home, 2026, 7).unwrap().len(), 1);
        assert_eq!(read_month(&home, 2026, 8).unwrap()[0].description, "a moved");
        delete_entry(&home, 2026, 7, 0).unwrap();
        assert!(read_month(&home, 2026, 7).unwrap().is_empty());
        assert!(matches!(delete_entry(&home, 2026, 7, 0), Err(StoreError::RowOutOfRange { .. })));
    }

    #[test]
    fn cross_month_move_writes_destination_before_source() {
        // Inject a failure into the SOURCE month's rewrite by occupying its
        // temp-file name with a directory, then check the row is already in
        // the destination month (duplicated, visible) rather than lost.
        let (_d, home) = temp_home();
        add_entry(&home, "a", "dev", "1", Some("2026-07-08")).unwrap();
        let blocker = home.log_dir.join(format!(".stint-2026-07.{}.tmp", std::process::id()));
        fs::create_dir(&blocker).unwrap();
        let err = update_entry(&home, 2026, 7, 0, entry("2026-08-01", "1", "a moved"));
        assert!(err.is_err());
        assert_eq!(read_month(&home, 2026, 8).unwrap()[0].description, "a moved");
        assert_eq!(read_month(&home, 2026, 7).unwrap().len(), 1, "source untouched");
    }

    #[test]
    fn delimiters_in_category_or_description_are_rejected() {
        let (_d, home) = temp_home();
        assert!(matches!(add_entry(&home, "x", "a,b", "1", Some("2026-09-01")), Err(StoreError::BadCategory(_))));
        assert!(matches!(add_entry(&home, "x", "a\"b", "1", Some("2026-09-01")), Err(StoreError::BadCategory(_))));
        assert!(matches!(start_timer(&home, "x", "c,d"), Err(StoreError::BadCategory(_))));
        assert!(matches!(start_timer(&home, "line1\nline2", "dev"), Err(StoreError::BadDescription)));
        assert!(read_month(&home, 2026, 9).unwrap().is_empty());
        assert!(read_timers(&home).unwrap().is_empty());
    }

    #[test]
    fn add_normalises_the_date() {
        let (_d, home) = temp_home();
        let e = add_entry(&home, "x", "dev", "1", Some("2026-9-2")).unwrap();
        assert_eq!(e.entry_date, "2026-09-02");
        assert_eq!(read_month(&home, 2026, 9).unwrap()[0].entry_date, "2026-09-02");
        assert!(matches!(add_entry(&home, "x", "dev", "1", Some("09/02/2026")), Err(StoreError::BadDate(_))));
    }

    #[test]
    fn timer_lifecycle_appends_engine_row() {
        let (_d, home) = temp_home();
        let t = start_timer(&home, "T1 thing", "pr").unwrap();
        assert_eq!(t.timer_id, 1);
        assert_eq!(read_timers(&home).unwrap().len(), 1);
        let t2 = start_timer(&home, "second", "dev").unwrap();
        assert_eq!(t2.timer_id, 2);
        // Force a deterministic end 40 minutes after start.
        let end = engine::format_ts(&(engine::parse_ts(&t.start).unwrap() + chrono::Duration::minutes(40)));
        let r = stop_timer(&home, &t, Some(&end)).unwrap();
        assert_eq!(r.hrs, "0.75");
        let (y, m) = split_ym(&t.entry_date).unwrap();
        let rows = read_month(&home, y, m).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, "pr");
        assert_eq!(rows[0].hrs, "0.75");
        assert_eq!(read_timers(&home).unwrap().len(), 1);
        assert_eq!(next_timer_id(&home), 1); // slot 1 is free again
    }

    #[test]
    fn summary_and_amounts() {
        let (_d, home) = temp_home();
        add_entry(&home, "x", "dev", "0.67", Some("2026-07-08")).unwrap(); // Wed
        add_entry(&home, "y", "dev", "1.00", Some("2026-07-06")).unwrap(); // Mon same week
        add_entry(&home, "z", "dev", "3.00", Some("2026-06-30")).unwrap(); // prev month
        let s = summary(&home, NaiveDate::from_ymd_opt(2026, 7, 8).unwrap()).unwrap();
        assert_eq!(s.today.hours.to_string(), "0.67");
        assert_eq!(s.today.amount.to_string(), "10.72");
        assert_eq!(s.week.hours.to_string(), "1.67");
        assert_eq!(s.month.hours.to_string(), "1.67");
    }

    #[test]
    fn available_months_newest_first() {
        let (_d, home) = temp_home();
        add_entry(&home, "a", "dev", "1", Some("2026-05-01")).unwrap();
        add_entry(&home, "b", "dev", "1", Some("2026-07-01")).unwrap();
        assert_eq!(available_months(&home), vec![(2026, 7), (2026, 5)]);
    }
}

#[cfg(test)]
mod legacy_prefix_tests {
    //! The ledger predates the rename, so months may be stored under either
    //! `stint-YYYY-MM.csv` or `savd-YYYY-MM.csv`. Readers accept both; a month
    //! that already exists under the legacy name keeps it, so nothing on a live
    //! ledger has to be migrated.

    use super::*;

    fn temp_home() -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        (dir, home)
    }

    #[test]
    fn a_legacy_month_keeps_its_filename() {
        let (_d, home) = temp_home();
        let legacy = home.log_dir.join("savd-2026-06.csv");
        fs::write(&legacy, format!("{}\n2026-06-01,manual,manual,1.50,dev,\"old\"\n", config::CSV_HEADER)).unwrap();

        assert_eq!(home.csv_path(2026, 6), legacy);
        let rows = read_month(&home, 2026, 6).unwrap();
        assert_eq!(rows.len(), 1);

        // Appending and rewriting stay in the legacy file; no new one appears.
        add_entry(&home, "added", "pr", "0.25", Some("2026-06-02")).unwrap();
        assert_eq!(read_month(&home, 2026, 6).unwrap().len(), 2);
        assert!(!home.log_dir.join("stint-2026-06.csv").exists());
        delete_entry(&home, 2026, 6, 0).unwrap();
        assert_eq!(read_month(&home, 2026, 6).unwrap().len(), 1);
        assert!(!home.log_dir.join("stint-2026-06.csv").exists());
    }

    #[test]
    fn a_new_month_uses_the_current_prefix() {
        let (_d, home) = temp_home();
        fs::write(home.log_dir.join("savd-2026-06.csv"), format!("{}\n", config::CSV_HEADER)).unwrap();
        add_entry(&home, "new month", "dev", "1", Some("2026-07-01")).unwrap();
        assert!(home.log_dir.join("stint-2026-07.csv").exists());
        assert!(!home.log_dir.join("savd-2026-07.csv").exists());
    }

    #[test]
    fn available_months_sees_both_prefixes_without_duplicates() {
        let (_d, home) = temp_home();
        for name in ["savd-2026-05.csv", "stint-2026-06.csv", "savd-2026-07.csv", "stint-2026-07.csv"] {
            fs::write(home.log_dir.join(name), format!("{}\n", config::CSV_HEADER)).unwrap();
        }
        assert_eq!(available_months(&home), vec![(2026, 7), (2026, 6), (2026, 5)]);
    }

    #[test]
    fn a_legacy_only_folder_still_looks_like_a_ledger() {
        let (_d, home) = temp_home();
        assert!(!home.looks_like_ledger());
        fs::write(home.log_dir.join("savd-2026-06.csv"), "x").unwrap();
        assert!(Home::new(&home.log_dir).looks_like_ledger());
    }
}
