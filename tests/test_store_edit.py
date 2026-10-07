"""store.py edit layer: add / update / delete via atomic CSV rewrite."""

from __future__ import annotations

import pytest

from stintcore import config, store


@pytest.fixture
def sandbox(tmp_path, monkeypatch):
    monkeypatch.setattr(config, "LOG_DIR", tmp_path)
    # store captures csv_path via config.LOG_DIR at call time, good.
    return tmp_path


def _seed(tmp_path):
    path = tmp_path / "stint-2026-08.csv"
    path.write_text(
        config.CSV_HEADER + "\n"
        '2026-08-01,09:00:00,10:00:00,1.00,dev,"first task"\n'
        '2026-08-02,manual,manual,0.5,admin,"second, with comma"\n'
    )
    return path


def test_read_roundtrip(sandbox):
    _seed(sandbox)
    rows = store.read_month(2026, 8)
    assert len(rows) == 2
    assert rows[1].description == "second, with comma"  # comma survives quoting
    assert rows[1].is_manual


def test_add_entry_appends(sandbox):
    _seed(sandbox)
    store.add_entry("backfilled work", "pr", "2.25", "2026-08-03")
    rows = store.read_month(2026, 8)
    assert len(rows) == 3
    assert rows[2].start_time == "manual" and rows[2].end_time == "manual"
    assert rows[2].hrs == "2.25" and rows[2].category == "pr"
    assert str(rows[2].amount) == "36.00"


def test_update_entry(sandbox):
    _seed(sandbox)
    store.update_entry(2026, 8, 0, description="edited task", category="docs", hrs="1.50")
    rows = store.read_month(2026, 8)
    assert rows[0].description == "edited task"
    assert rows[0].category == "docs"
    assert rows[0].hrs == "1.50"


def test_delete_entry(sandbox):
    _seed(sandbox)
    removed = store.delete_entry(2026, 8, 0)
    rows = store.read_month(2026, 8)
    assert removed.description == "first task"
    assert len(rows) == 1
    assert rows[0].description == "second, with comma"


def test_update_moves_across_month(sandbox):
    _seed(sandbox)
    # Change the date into September — row should leave August, land in September.
    store.update_entry(2026, 8, 0, entry_date="2026-09-15")
    assert len(store.read_month(2026, 8)) == 1
    sept = store.read_month(2026, 9)
    assert len(sept) == 1 and sept[0].entry_date == "2026-09-15"


def test_embedded_quote_escaped(sandbox):
    """The latent bash bug: a description with a `"` must round-trip, not corrupt."""
    _seed(sandbox)
    store.add_entry('fixed the "auth" bug', "dev", "0.75", "2026-08-04")
    # Re-read: csv parser must recover the exact description.
    rows = store.read_month(2026, 8)
    assert rows[-1].description == 'fixed the "auth" bug'
    # And the raw line must use RFC-4180 doubled quotes.
    raw = (sandbox / "stint-2026-08.csv").read_text().splitlines()[-1]
    assert '"fixed the ""auth"" bug"' in raw


def test_write_is_atomic_no_partial(sandbox, monkeypatch):
    _seed(sandbox)
    original = (sandbox / "stint-2026-08.csv").read_text()

    # Force os.replace to fail; the original file must be left intact.
    import stintcore.store as s

    monkeypatch.setattr(s.os, "replace", lambda *a, **k: (_ for _ in ()).throw(OSError("boom")))
    with pytest.raises(OSError):
        store.delete_entry(2026, 8, 0)
    assert (sandbox / "stint-2026-08.csv").read_text() == original
    # No leftover temp files.
    assert not list(sandbox.glob(".stint-*.tmp"))


# ── legacy CSV prefix ────────────────────────────────────────────────────────
# The ledger predates the savd -> stint rename, so a month may be stored as
# either `stint-YYYY-MM.csv` or `savd-YYYY-MM.csv`. Readers accept both and a
# month already under the legacy name keeps it, so a live ledger needs no
# migration and the Bash / Python / Rust paths agree on which file a month is in.


def test_legacy_month_keeps_its_filename(tmp_path, monkeypatch):
    from stintcore import config, store

    monkeypatch.setattr(config, "LOG_DIR", tmp_path)
    legacy = tmp_path / "savd-2026-06.csv"
    legacy.write_text(config.CSV_HEADER + '\n2026-06-01,manual,manual,1.50,dev,"old"\n', encoding="utf-8")

    assert store.csv_path(2026, 6) == legacy
    assert len(store.read_month(2026, 6)) == 1

    store.add_entry("added", "pr", "0.25", "2026-06-02")
    assert len(store.read_month(2026, 6)) == 2
    assert not (tmp_path / "stint-2026-06.csv").exists()

    store.delete_entry(2026, 6, 0)
    assert len(store.read_month(2026, 6)) == 1
    assert not (tmp_path / "stint-2026-06.csv").exists()


def test_new_month_uses_the_current_prefix(tmp_path, monkeypatch):
    from stintcore import config, store

    monkeypatch.setattr(config, "LOG_DIR", tmp_path)
    (tmp_path / "savd-2026-06.csv").write_text(config.CSV_HEADER + "\n", encoding="utf-8")
    store.add_entry("new month", "dev", "1", "2026-07-01")
    assert (tmp_path / "stint-2026-07.csv").exists()
    assert not (tmp_path / "savd-2026-07.csv").exists()


def test_available_months_sees_both_prefixes(tmp_path, monkeypatch):
    from stintcore import config, store

    monkeypatch.setattr(config, "LOG_DIR", tmp_path)
    for name in ("savd-2026-05.csv", "stint-2026-06.csv", "savd-2026-07.csv", "stint-2026-07.csv"):
        (tmp_path / name).write_text(config.CSV_HEADER + "\n", encoding="utf-8")
    assert store.available_months() == [(2026, 7), (2026, 6), (2026, 5)]
