"""M3: entry badges + report rollup helpers (pure, no running app)."""

from __future__ import annotations

from collections import OrderedDict
from datetime import timedelta
from decimal import Decimal

from stintcore import config, store
from stintcore.tui.app import _entry_badges


def _e(**kw) -> store.Entry:
    base = dict(
        entry_date="2026-08-01", start_time="09:00:00", end_time="10:00:00",
        hrs="1.00", category="dev", description="x",
    )
    base.update(kw)
    return store.Entry(**base)


def test_badge_manual():
    e = _e(start_time="manual", end_time="manual", hrs="0.50")
    assert _entry_badges(e).strip() == "✎"


def test_badge_roundup():
    # 5 minutes measured = 0.08h, billed 0.25h → rounded up to the next quarter.
    e = _e(start_time="09:00:00", end_time="09:05:00", hrs="0.25", category="pr")
    assert "+¼" in _entry_badges(e)


def test_badge_none_for_normal_measured():
    # A full hour billed as measured → no badge.
    e = _e(start_time="09:00:00", end_time="10:00:00", hrs="1.00")
    assert _entry_badges(e).strip() == ""


def test_report_category_and_week_grouping(tmp_path, monkeypatch):
    monkeypatch.setattr(config, "LOG_DIR", tmp_path)
    (tmp_path / "stint-2026-08.csv").write_text(
        config.CSV_HEADER + "\n"
        '2026-08-03,09:00:00,10:00:00,1.00,dev,"a"\n'   # week of Aug 3 (Mon)
        '2026-08-04,09:00:00,10:00:00,2.00,pr,"b"\n'    # same week
        '2026-08-11,09:00:00,10:00:00,1.50,dev,"c"\n'   # week of Aug 10 (Mon)
    )
    entries = store.read_month(2026, 8)

    # Category rollup
    by_cat: "OrderedDict[str, list]" = OrderedDict()
    for e in entries:
        by_cat.setdefault(e.category, []).append(e)
    dev_h = sum((e.hours for e in by_cat["dev"]), Decimal(0))
    pr_h = sum((e.hours for e in by_cat["pr"]), Decimal(0))
    assert dev_h == Decimal("2.50")
    assert pr_h == Decimal("2.00")

    # Weekly rollup (Monday-keyed)
    weeks: dict[str, Decimal] = {}
    for e in entries:
        monday = e.date - timedelta(days=e.date.weekday())
        weeks[monday.isoformat()] = weeks.get(monday.isoformat(), Decimal(0)) + e.hours
    assert weeks["2026-08-03"] == Decimal("3.00")
    assert weeks["2026-08-10"] == Decimal("1.50")
