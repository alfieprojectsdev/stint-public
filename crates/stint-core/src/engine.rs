//! Timer / billing engine, ported from `stintcore/engine.py` (itself a port of
//! the Bash `stint.sh` `_stop_timer_file`).
//!
//! Correctness contract: for the same inputs, [`stop`] produces a CSV line
//! byte-identical to Bash for every value EXCEPT the documented half-cent-hour
//! boundaries where legacy Bash `printf` double-rounds (see [`printf_2f`]).
//! The pytest suite (`tests/test_parity.py` + `tests/test_rust_parity.py`)
//! runs the Bash oracle against both the Python and the Rust engine.
//!
//! Fidelity notes:
//!   * `duration_hrs` mirrors `bc "scale=4; diff/3600"` (truncation to 4 dp)
//!     followed by `printf "%.2f"`. The truncation is integer math; the final
//!     formatting goes through `format!("{:.2}", f64)`, which (like Python's
//!     `"%.2f"`) rounds the exact binary value half-to-even. Checked against
//!     Python on every tie in the grid before this was written.
//!   * The quarter-hour rule works on whole seconds in 900s buckets, matching
//!     Bash `(diff + 899) / 900`.
//!   * The CSV description is wrapped in double quotes with NO escaping,
//!     exactly like Bash `\"$description\"`. `store::serialize_row` is the
//!     RFC 4180-escaping writer used for rewrites; this one is the append path.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use serde::Serialize;

use crate::config;

pub const TS_FMT: &str = "%Y-%m-%d %H:%M:%S";

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("bad timestamp {0:?} (expected YYYY-MM-DD HH:MM:SS)")]
    BadTimestamp(String),
}

pub fn parse_ts(ts: &str) -> Result<NaiveDateTime, EngineError> {
    NaiveDateTime::parse_from_str(ts, TS_FMT).map_err(|_| EngineError::BadTimestamp(ts.to_string()))
}

pub fn format_ts(dt: &NaiveDateTime) -> String {
    dt.format(TS_FMT).to_string()
}

/// Local-time epoch of a naive timestamp (Python `datetime.timestamp()` on a
/// naive value). DST gaps/overlaps take the earliest reading; out of scope
/// for the original too.
pub fn local_epoch(dt: &NaiveDateTime) -> i64 {
    Local
        .from_local_datetime(dt)
        .earliest()
        .map(|d| d.timestamp())
        .unwrap_or_else(|| dt.and_utc().timestamp())
}

pub fn epoch_to_local_ts(epoch: i64) -> String {
    let dt: DateTime<Local> = Local.timestamp_opt(epoch, 0).earliest().unwrap_or_default();
    dt.format(TS_FMT).to_string()
}

/// Whole seconds between two timestamps (Bash `$(( end_epoch - start_epoch ))`).
pub fn duration_seconds(start: &str, end: &str) -> Result<i64, EngineError> {
    Ok((parse_ts(end)? - parse_ts(start)?).num_seconds())
}

/// `printf "%.2f"` of a value given in ten-thousandths (the `bc scale=4`
/// truncated quotient). Feeds the same double to the same kind of correctly
/// rounded formatter Python uses, so the last digit matches exactly.
pub fn printf_2f(ten_thousandths: i64) -> String {
    let v = ten_thousandths as f64 / 10000.0;
    format!("{v:.2}")
}

/// Port of Bash `duration_hrs` (stint.sh:52-60): measured hours, 2 dp.
pub fn duration_hrs(start: &str, end: &str) -> Result<String, EngineError> {
    let diff = duration_seconds(start, end)?;
    // bc "scale=4; diff / 3600" truncates toward zero; so does i64 division.
    Ok(printf_2f(diff * 10000 / 3600))
}

/// Quarter-hour round-up rule (Bash `_stop_timer_file` block).
///
/// Measured session time is rounded UP to the next 15-minute mark: any
/// positive session under 15m bills 0.25h, 15m01s..30m bills 0.50h, and so
/// on; a session landing exactly on a boundary is unchanged. Stopwatch entries
/// only; manual `add` entries never pass through here.
pub fn round_up_to_quarter(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let quarters = (seconds + config::QUARTER_HOUR_SECS - 1) / config::QUARTER_HOUR_SECS;
    let quarters = quarters.max(1);
    // quarters * 0.25h, in ten-thousandths of an hour.
    printf_2f(quarters * 2500)
}

/// Earliest system boot strictly after `start_epoch`, or None.
///
/// `boots` is injectable for tests; `None` shells out to `last` like Bash.
pub fn first_boot_after(start_epoch: i64, boots: Option<&[i64]>) -> Option<i64> {
    let owned;
    let boots = match boots {
        Some(b) => b,
        None => {
            owned = system_boot_epochs();
            &owned
        }
    };
    boots.iter().copied().filter(|&b| b > start_epoch).min()
}

/// Boot timestamps as epochs: every reboot `last --time-format iso reboot`
/// knows about (Linux), plus the current boot from the OS uptime counter on
/// Windows (`GetTickCount64`), where `last` doesn't exist. A Windows reboot
/// also takes WSL and its timer files down, so capping at it is right.
pub fn system_boot_epochs() -> Vec<i64> {
    let mut epochs = Vec::new();
    #[cfg(windows)]
    {
        extern "system" {
            fn GetTickCount64() -> u64;
        }
        // SAFETY: plain kernel32 call with no arguments.
        let uptime_ms = unsafe { GetTickCount64() };
        let now = chrono::Utc::now().timestamp();
        epochs.push(now - (uptime_ms / 1000) as i64);
    }
    let out = match std::process::Command::new("last")
        .args(["--time-format", "iso", "reboot"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return epochs,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 && parts[0] == "reboot" {
            if let Ok(dt) = DateTime::parse_from_rfc3339(parts[4]) {
                epochs.push(dt.timestamp());
            }
        }
    }
    epochs
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StopResult {
    pub entry_date: String,
    /// Full `YYYY-MM-DD HH:MM:SS`.
    pub start_time: String,
    /// Possibly reboot-capped.
    pub end_time: String,
    /// Measured hours before the quarter-hour rule (2 dp), for the stop message.
    pub measured_hrs: String,
    /// Billable hours, 2 dp.
    pub hrs: String,
    pub category: String,
    pub description: String,
    pub reboot_capped: bool,
}

impl StopResult {
    /// Byte-identical to Bash stint.sh:138.
    pub fn csv_row(&self) -> String {
        let start_hms = hms(&self.start_time);
        let end_hms = hms(&self.end_time);
        format!(
            "{},{},{},{},{},\"{}\"",
            self.entry_date, start_hms, end_hms, self.hrs, self.category, self.description
        )
    }

    /// Reproduce Bash `bc "scale=2; hrs*RATE"` (truncated to cents).
    pub fn billable(&self) -> String {
        billable_for_hrs(&self.hrs)
    }

    /// True when the quarter-hour rule changed the billed figure.
    pub fn rounded(&self) -> bool {
        self.hrs != self.measured_hrs
    }
}

fn hms(ts: &str) -> &str {
    ts.split_once(' ').map(|(_, t)| t).unwrap_or(ts)
}

/// `bc "scale=2; hrs * 16.00"`: exact decimal product truncated to cents.
/// Works for any decimal string bc would accept (`0.5`, `2.20`, `100.00`).
pub fn billable_for_hrs(hrs: &str) -> String {
    use rust_decimal::{Decimal, RoundingStrategy};
    let h: Decimal = hrs.trim().parse().unwrap_or_default();
    let amount = (h * config::RATE).round_dp_with_strategy(2, RoundingStrategy::ToZero);
    format!("{amount:.2}")
}

/// Full port of `_stop_timer_file` minus the file I/O.
///
/// Applies the reboot cap then the quarter-hour round-up, in that order.
/// `end_time` defaults to now; `boots` is injectable for deterministic tests
/// (`Some(&[])` = no cap, matching the oracle's assumption).
pub fn stop(
    entry_date: &str,
    start_time: &str,
    category: &str,
    description: &str,
    end_time: Option<&str>,
    boots: Option<&[i64]>,
) -> Result<StopResult, EngineError> {
    let mut end_time = match end_time {
        Some(e) => e.to_string(),
        None => format_ts(&Local::now().naive_local()),
    };

    let start_epoch = local_epoch(&parse_ts(start_time)?);
    let end_epoch = local_epoch(&parse_ts(&end_time)?);

    let mut reboot_capped = false;
    if let Some(boot) = first_boot_after(start_epoch, boots) {
        if boot < end_epoch {
            end_time = epoch_to_local_ts(boot);
            reboot_capped = true;
        }
    }

    let diff = duration_seconds(start_time, &end_time)?;
    let measured_hrs = printf_2f(diff * 10000 / 3600);
    let hrs = round_up_to_quarter(diff);

    Ok(StopResult {
        entry_date: entry_date.to_string(),
        start_time: start_time.to_string(),
        end_time,
        measured_hrs,
        hrs,
        category: category.to_string(),
        description: description.to_string(),
        reboot_capped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "2026-07-08 09:00:00";

    fn end_after(secs: i64) -> String {
        format_ts(&(parse_ts(START).unwrap() + chrono::Duration::seconds(secs)))
    }

    #[test]
    fn duration_hrs_truncates_then_rounds_like_bash() {
        // Values cross-checked against Python stintcore.engine.duration_hrs.
        for (secs, want) in [
            (0, "0.00"), (1, "0.00"), (59, "0.02"), (60, "0.02"), (89, "0.02"), (90, "0.03"),
            (162, "0.04"), // 0.045h: correct half-even, legacy bash says 0.05
            (450, "0.12"), // 0.125h exact tie -> even
            (899, "0.25"), (900, "0.25"), (901, "0.25"), (1234, "0.34"), (3599, "1.00"),
            (3600, "1.00"), (5000, "1.39"), (43200, "12.00"),
        ] {
            assert_eq!(duration_hrs(START, &end_after(secs)).unwrap(), want, "{secs}s");
        }
    }

    #[test]
    fn quarter_roundup_boundaries() {
        assert_eq!(round_up_to_quarter(0), "0.25");
        assert_eq!(round_up_to_quarter(1), "0.25");
        assert_eq!(round_up_to_quarter(899), "0.25");
        assert_eq!(round_up_to_quarter(900), "0.25");
        assert_eq!(round_up_to_quarter(901), "0.50");
        assert_eq!(round_up_to_quarter(1800), "0.50");
        assert_eq!(round_up_to_quarter(1801), "0.75");
        assert_eq!(round_up_to_quarter(3600), "1.00");
        assert_eq!(round_up_to_quarter(43200), "12.00");
        assert_eq!(round_up_to_quarter(-5), "0.25");
    }

    #[test]
    fn csv_row_matches_bash_shape() {
        let r = stop("2026-07-08", START, "pr", "PR #129 review — diff, audit", Some(&end_after(2400)), Some(&[])).unwrap();
        assert_eq!(r.csv_row(), "2026-07-08,09:00:00,09:40:00,0.75,pr,\"PR #129 review — diff, audit\"");
        assert_eq!(r.measured_hrs, "0.67");
        assert!(r.rounded());
        assert_eq!(r.billable(), "12.00");
    }

    #[test]
    fn billable_truncates_to_cents() {
        for (hrs, want) in [("0.25", "4.00"), ("0.67", "10.72"), ("1.07", "17.12"), ("6.18", "98.88"), ("42.10", "673.60"), ("0.10", "1.60"), ("100.00", "1600.00"), ("0.5", "8.00")] {
            assert_eq!(billable_for_hrs(hrs), want, "{hrs}");
        }
    }

    #[test]
    fn reboot_cap_uses_first_boot_after_start() {
        let boot = local_epoch(&parse_ts("2026-07-08 09:30:00").unwrap());
        let r = stop("2026-07-08", START, "dev", "spanned a reboot", Some("2026-07-08 17:00:00"), Some(&[boot])).unwrap();
        assert!(r.reboot_capped);
        assert_eq!(r.end_time, "2026-07-08 09:30:00");
        assert_eq!(r.hrs, "0.50");
    }

    #[test]
    fn first_boot_after_selects_earliest() {
        assert_eq!(first_boot_after(100, Some(&[50, 150, 200, 175])), Some(150));
        assert_eq!(first_boot_after(100, Some(&[50, 90])), None);
        assert_eq!(first_boot_after(100, Some(&[])), None);
    }
}
