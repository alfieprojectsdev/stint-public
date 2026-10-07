//! Read-only rollups and transparency badges, shared by every front-end
//! (ported from the Textual TUI's `refresh_report` and `_entry_badges`).

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::config;
use crate::engine;
use crate::store::{self, Entry};

/// Transparency badges: `✎` manual entry, `+¼` quarter-hour round-up.
///
/// `+¼` is inferred by comparing the stored billable hours with the raw
/// measured duration: a stopwatch entry billed above measured was rounded
/// up. A reboot cap can't be inferred from a stored row, so it's surfaced
/// only at stop time.
pub fn badges(e: &Entry) -> Vec<&'static str> {
    let mut out = Vec::new();
    if e.is_manual() {
        out.push("✎");
    } else if let Ok(measured) = engine::duration_hrs(
        &format!("{} {}", e.entry_date, e.start_time),
        &format!("{} {}", e.entry_date, e.end_time),
    ) {
        let measured: Decimal = measured.parse().unwrap_or_default();
        if e.hours() > measured {
            out.push("+¼");
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryRow {
    pub category: String,
    pub entries: usize,
    pub hours: Decimal,
    pub amount: Decimal,
    /// Share of the month's hours, whole percent.
    pub share_pct: Decimal,
}

/// Per-category rollup in `config::CATEGORIES` order, unknown categories last.
pub fn by_category(entries: &[Entry]) -> Vec<CategoryRow> {
    let total_h: Decimal = entries.iter().map(Entry::hours).sum();
    let mut order: Vec<String> = config::CATEGORIES.iter().map(|c| c.to_string()).collect();
    for e in entries {
        if !order.contains(&e.category) {
            order.push(e.category.clone());
        }
    }
    order
        .into_iter()
        .filter_map(|cat| {
            let rows: Vec<&Entry> = entries.iter().filter(|e| e.category == cat).collect();
            if rows.is_empty() {
                return None;
            }
            let hours: Decimal = rows.iter().map(|e| e.hours()).sum();
            let amount: Decimal = rows.iter().map(|e| e.amount()).sum();
            let share_pct = if total_h.is_zero() { Decimal::ZERO } else { (hours / total_h * Decimal::from(100)).round() };
            Some(CategoryRow { category: cat, entries: rows.len(), hours, amount, share_pct })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WeekRow {
    pub monday: NaiveDate,
    pub sunday: NaiveDate,
    pub entries: usize,
    pub hours: Decimal,
    pub amount: Decimal,
}

impl WeekRow {
    /// `Jun 1–7` style span, like the TUI.
    pub fn span(&self) -> String {
        format!("{} {}–{}", self.monday.format("%b"), self.monday.day(), self.sunday.day())
    }
}

/// Per ISO week (Mon–Sun) rollup, keyed on the Monday, in date order.
pub fn by_week(entries: &[Entry]) -> Vec<WeekRow> {
    let mut dated: Vec<(NaiveDate, &Entry)> = entries.iter().filter_map(|e| e.date().map(|d| (d, e))).collect();
    dated.sort_by_key(|(d, _)| *d);
    let mut weeks: Vec<WeekRow> = Vec::new();
    for (d, e) in dated {
        let monday = d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64);
        match weeks.iter_mut().find(|w| w.monday == monday) {
            Some(w) => {
                w.entries += 1;
                w.hours += e.hours();
                w.amount += e.amount();
            }
            None => weeks.push(WeekRow {
                monday,
                sunday: monday + chrono::Duration::days(6),
                entries: 1,
                hours: e.hours(),
                amount: e.amount(),
            }),
        }
    }
    weeks
}

/// Month totals shortcut for headers.
pub fn month_totals(entries: &[Entry]) -> store::Totals {
    store::sum(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(date: &str, start: &str, end: &str, hrs: &str, cat: &str) -> Entry {
        Entry { entry_date: date.into(), start_time: start.into(), end_time: end.into(), hrs: hrs.into(), category: cat.into(), description: "x".into() }
    }

    #[test]
    fn badges_infer_manual_and_roundup() {
        assert_eq!(badges(&e("2026-06-04", "manual", "manual", "1.50", "research")), vec!["✎"]);
        assert_eq!(badges(&e("2026-06-02", "10:15:11", "10:19:58", "0.25", "pr")), vec!["+¼"]);
        assert!(badges(&e("2026-06-01", "09:00:00", "11:15:00", "2.25", "dev")).is_empty());
    }

    #[test]
    fn category_and_week_rollups() {
        let es = vec![
            e("2026-06-01", "manual", "manual", "2.00", "dev"), // Mon
            e("2026-06-03", "manual", "manual", "1.00", "pr"),  // Wed same week
            e("2026-06-09", "manual", "manual", "1.00", "dev"), // next week
        ];
        let cats = by_category(&es);
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].category, "pr"); // config order: pr before dev
        assert_eq!(cats[1].hours.to_string(), "3.00");
        assert_eq!(cats[1].share_pct.to_string(), "75");
        let weeks = by_week(&es);
        assert_eq!(weeks.len(), 2);
        assert_eq!(weeks[0].span(), "Jun 1–7");
        assert_eq!(weeks[0].entries, 2);
        assert_eq!(weeks[1].hours.to_string(), "1.00");
    }
}

// ── Activity distribution (heatmaps) ─────────────────────────────────────────

/// Hours per calendar day, ascending. Manual entries count toward their day.
pub fn daily_hours(entries: &[Entry]) -> std::collections::BTreeMap<NaiveDate, Decimal> {
    daily_totals(entries).into_iter().map(|(d, t)| (d, t.hours)).collect()
}

/// Hours and billable amount per calendar day. The amount is the sum of the
/// rows' own (cent-truncated) amounts, so it agrees with the month totals
/// rather than being re-derived from hours x rate.
pub fn daily_totals(entries: &[Entry]) -> std::collections::BTreeMap<NaiveDate, store::Totals> {
    let mut out: std::collections::BTreeMap<NaiveDate, store::Totals> = std::collections::BTreeMap::new();
    for e in entries {
        if let Some(d) = e.date() {
            let t = out.entry(d).or_default();
            t.hours += e.hours();
            t.amount += e.amount();
        }
    }
    out
}

/// Hours per weekday (index 0 = Monday). Manual entries count.
pub fn weekday_hours(entries: &[Entry]) -> [Decimal; 7] {
    let mut out = [Decimal::ZERO; 7];
    for e in entries {
        if let Some(d) = e.date() {
            out[d.weekday().num_days_from_monday() as usize] += e.hours();
        }
    }
    out
}

/// Hours per hour-of-day (index 0 = 00:00-01:00), spreading each stopwatch
/// entry across the clock hours it overlaps by measured wall time. Manual
/// entries have no clock time and are skipped; the billed (quarter-rounded)
/// hours are scaled onto the measured span so the buckets sum to the billed
/// total, not the measured one.
pub fn hour_of_day_hours(entries: &[Entry]) -> [Decimal; 24] {
    use chrono::{NaiveTime, Timelike};
    let mut out = [Decimal::ZERO; 24];
    for e in entries {
        if e.is_manual() {
            continue;
        }
        let (Ok(s), Ok(t)) = (
            NaiveTime::parse_from_str(&e.start_time, "%H:%M:%S"),
            NaiveTime::parse_from_str(&e.end_time, "%H:%M:%S"),
        ) else {
            continue;
        };
        let start = s.num_seconds_from_midnight() as i64;
        let mut end = t.num_seconds_from_midnight() as i64;
        if end < start {
            end += 86_400; // crossed midnight
        }
        let span = end - start;
        let billed = e.hours();
        if span <= 0 {
            // Zero-length session (billed a minimum quarter): put it in its hour.
            out[(start / 3600) as usize % 24] += billed;
            continue;
        }
        let mut cur = start;
        while cur < end {
            let bucket_end = ((cur / 3600) + 1) * 3600;
            let piece_end = bucket_end.min(end);
            let frac = Decimal::from(piece_end - cur) / Decimal::from(span);
            out[((cur / 3600) % 24) as usize] += billed * frac;
            cur = piece_end;
        }
    }
    out
}

#[cfg(test)]
mod heatmap_tests {
    use super::*;

    fn e(date: &str, start: &str, end: &str, hrs: &str) -> Entry {
        Entry { entry_date: date.into(), start_time: start.into(), end_time: end.into(), hrs: hrs.into(), category: "dev".into(), description: "x".into() }
    }

    #[test]
    fn daily_and_weekday_sums() {
        let es = vec![e("2026-06-01", "manual", "manual", "1.00"), e("2026-06-01", "09:00:00", "10:00:00", "1.00"), e("2026-06-02", "09:00:00", "09:30:00", "0.50")];
        let d = daily_hours(&es);
        assert_eq!(d[&NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()].to_string(), "2.00");
        let w = weekday_hours(&es);
        assert_eq!(w[0].to_string(), "2.00"); // Mon
        assert_eq!(w[1].to_string(), "0.50"); // Tue
    }

    #[test]
    fn hour_of_day_splits_across_buckets_and_keeps_billed_total() {
        // 09:30-11:30 billed 2.00 -> 0.5 in 09, 1.0 in 10, 0.5 in 11.
        let es = vec![e("2026-06-01", "09:30:00", "11:30:00", "2.00")];
        let h = hour_of_day_hours(&es);
        assert_eq!(h[9], Decimal::new(50, 2));
        assert_eq!(h[10], Decimal::new(100, 2));
        assert_eq!(h[11], Decimal::new(50, 2));
        // Quarter-rounded 0.25 over a 5-minute span still sums to 0.25.
        let es = vec![e("2026-06-01", "10:57:00", "11:02:00", "0.25")];
        let h = hour_of_day_hours(&es);
        let total: Decimal = h.iter().sum();
        assert_eq!(total, Decimal::new(25, 2));
        // Manual entries have no clock position.
        let es = vec![e("2026-06-01", "manual", "manual", "3.00")];
        assert!(hour_of_day_hours(&es).iter().all(|v| v.is_zero()));
        // Crossing midnight.
        let es = vec![e("2026-06-01", "23:30:00", "00:30:00", "1.00")];
        let h = hour_of_day_hours(&es);
        assert_eq!(h[23], Decimal::new(50, 2));
        assert_eq!(h[0], Decimal::new(50, 2));
    }
}
