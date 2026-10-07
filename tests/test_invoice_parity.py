"""Byte-parity: stintcore.invoice must reproduce legacy legacy-consolidate.py.

The legacy script is the contract for invoice output. For the default client and
the same CSV, the ported module must emit identical line items, staging text,
and HTML — otherwise regenerating a past invoice would silently change it.
"""

from __future__ import annotations

import importlib.util
import shutil
from datetime import date
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parent.parent
FIXED_INV = 1004  # pin so HTML is deterministic (only the counter is non-deterministic)


def _load_legacy():
    spec = importlib.util.spec_from_file_location("legacy_consolidate", REPO / "legacy-consolidate.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _month_files(folder: Path) -> list[Path]:
    """Ledger CSVs under either prefix, newest month first."""
    files = list(folder.glob("stint-????-??.csv")) + list(folder.glob("savd-????-??.csv"))
    return sorted(files, key=lambda p: p.stem.split("-", 1)[1], reverse=True)


def _pick_month() -> tuple[int, int]:
    for p in _month_files(REPO):
        parts = p.stem.split("-")
        return int(parts[1]), int(parts[2])
    pytest.skip("no YYYY-MM ledger CSVs present")


@pytest.fixture
def sandbox(tmp_path, monkeypatch):
    """A tmp LOG_DIR with the real template + newest CSV copied in."""
    (tmp_path / "temp").mkdir()
    shutil.copy(REPO / "temp" / "invoice-dynamic.html", tmp_path / "temp" / "invoice-dynamic.html")
    year, month = _pick_month()
    src = next(p for p in _month_files(REPO) if p.stem.endswith(f"{year}-{month:02d}"))
    shutil.copy(src, tmp_path / src.name)

    from stintcore import invoice

    monkeypatch.setattr(invoice, "LOG_DIR", tmp_path)
    monkeypatch.setattr(invoice, "TEMPLATE_FILE", tmp_path / "temp" / "invoice-dynamic.html")
    return tmp_path, year, month


def _run_legacy_html(tmp_path, year, month, monkeypatch):
    legacy = _load_legacy()
    monkeypatch.setattr(legacy, "LOG_DIR", tmp_path)
    monkeypatch.setattr(legacy, "TEMPLATE_FILE", tmp_path / "temp" / "invoice-dynamic.html")
    monkeypatch.setattr(legacy, "COUNTER_FILE", tmp_path / ".invoice-counter")
    monkeypatch.setattr(legacy, "next_invoice_number", lambda: FIXED_INV)
    monkeypatch.setattr(legacy, "commit_invoice_number", lambda cur: None)

    entries = legacy.load_entries(year, month)
    groups = legacy.group_entries(entries)
    items = [legacy.build_line_item(lt, lk, ents) for (lt, lk), ents in groups.items()]
    legacy.render_html(items, entries, year, month)
    return (tmp_path / "temp" / f"invoice-{year}-{month:02d}.html").read_text()


def test_line_items_match_legacy(sandbox, monkeypatch):
    tmp_path, year, month = sandbox
    from stintcore import invoice

    legacy = _load_legacy()
    monkeypatch.setattr(legacy, "LOG_DIR", tmp_path)
    legacy_entries = legacy.load_entries(year, month)
    legacy_groups = legacy.group_entries(legacy_entries)
    legacy_items = [legacy.build_line_item(lt, lk, e) for (lt, lk), e in legacy_groups.items()]

    items, _raw = invoice.line_items_for(year, month)

    assert len(items) == len(legacy_items)
    for got, exp in zip(items, legacy_items):
        assert got.label == exp["label"]
        assert got.narrative == exp["narrative"]
        assert got.hours == exp["hours"]
        assert got.total == exp["total"]


def _staging_body(text: str) -> list[str]:
    """Staging lines minus the '# Generated: <date>' stamp (the only non-deterministic bit)."""
    return [ln for ln in text.splitlines() if not ln.startswith("# Generated:")]


def test_staging_matches_legacy(sandbox, monkeypatch):
    tmp_path, year, month = sandbox
    from stintcore import invoice

    legacy = _load_legacy()
    monkeypatch.setattr(legacy, "LOG_DIR", tmp_path)

    legacy_entries = legacy.load_entries(year, month)
    legacy_groups = legacy.group_entries(legacy_entries)
    legacy_items = [legacy.build_line_item(lt, lk, e) for (lt, lk), e in legacy_groups.items()]
    legacy_path = legacy.write_staging(legacy_items, year, month)  # writes into tmp/temp

    items, _raw = invoice.line_items_for(year, month)
    got_path = invoice.write_staging(items, year, month, today=date(2026, 7, 10))

    assert _staging_body(got_path.read_text()) == _staging_body(legacy_path.read_text())


def test_html_byte_identical_to_legacy(sandbox, monkeypatch):
    tmp_path, year, month = sandbox
    from stintcore import invoice

    legacy_html = _run_legacy_html(tmp_path, year, month, monkeypatch)

    entries = invoice.load_entries(year, month)
    items, _raw = invoice.line_items_for(year, month)
    from stintcore import config

    got_html = invoice.build_html(items, entries, year, month, inv_num=FIXED_INV, client_key=config.DEFAULT_CLIENT)

    assert got_html == legacy_html


def test_manual_client_html_refused(sandbox):
    tmp_path, year, month = sandbox
    from stintcore import config, invoice

    entries = invoice.load_entries(year, month)
    items, _raw = invoice.line_items_for(year, month)
    with pytest.raises(ValueError, match="not HTML-capable"):
        invoice.build_html(items, entries, year, month, inv_num=2002, client_key=config.manual_client().key)


def test_counter_selects_by_client(tmp_path, monkeypatch):
    from stintcore import config, invoice

    monkeypatch.setattr(config, "LOG_DIR", tmp_path)
    # Client.counter_path reads config.LOG_DIR at call time.
    main = config.DEFAULT_CLIENT
    assert invoice.next_invoice_number(main) == config.CLIENTS[main].counter_seed
    invoice.commit_invoice_number(config.CLIENTS[main].counter_seed, main)
    assert invoice.next_invoice_number(main) == config.CLIENTS[main].counter_seed + 1
    # The hand-built client uses its own file — untouched by the commits above.
    manual = config.manual_client()
    assert manual is not None
    assert invoice.next_invoice_number(manual.key) == manual.counter_seed
