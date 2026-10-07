"""Rust invoice parity: `stint parity items|staging|html` must be byte-identical
to `stintcore.invoice` (itself byte-identical to legacy legacy-consolidate.py).

Runs against the committed `samples/` dataset always, and additionally against
the newest real `stint-YYYY-MM.csv` when one is present (the live ledger). The
binary is located like test_rust_parity.py; missing -> module skipped.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from datetime import date
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parent.parent
FIXED_INV = 1004


def _find_binary() -> Path | None:
    env = os.environ.get("STINT_BIN")
    if env and Path(env).exists():
        return Path(env)
    for profile in ("debug", "release"):
        for name in ("stint", "stint.exe"):
            p = REPO / "target" / profile / name
            if p.exists():
                return p
    return None


_BIN = _find_binary()
pytestmark = pytest.mark.skipif(_BIN is None, reason="stint binary not built (cargo build)")


def _month_files(folder: Path) -> list[Path]:
    """Ledger CSVs under either prefix (stint- or pre-rename savd-), newest first."""
    files = list(folder.glob("stint-????-??.csv")) + list(folder.glob("savd-????-??.csv"))
    return sorted(files, key=lambda p: p.stem.split("-", 1)[1], reverse=True)


def _datasets() -> list[tuple[Path, int, int]]:
    out = []
    for folder in (REPO / "samples", REPO):
        for p in _month_files(folder)[:1]:
            _, y, m = p.stem.split("-")
            out.append((folder, int(y), int(m)))
    return out


@pytest.fixture(params=_datasets(), ids=lambda d: f"{d[0].name}-{d[1]}-{d[2]:02d}")
def sandbox(request, tmp_path, monkeypatch):
    src, year, month = request.param
    (tmp_path / "temp").mkdir()
    template = src / "temp" / "invoice-dynamic.html"
    if not template.exists():
        template = REPO / "samples" / "temp" / "invoice-dynamic.html"
    shutil.copy(template, tmp_path / "temp" / "invoice-dynamic.html")
    csv = next(p for p in _month_files(src) if p.stem.endswith(f"{year}-{month:02d}"))
    shutil.copy(csv, tmp_path / csv.name)
    # The Python core loaded clients.json from the repo root at import time; give
    # the Rust binary the same file so both render the same Bill-To block.
    if (REPO / "clients.json").exists():
        shutil.copy(REPO / "clients.json", tmp_path / "clients.json")

    from stintcore import invoice

    monkeypatch.setattr(invoice, "LOG_DIR", tmp_path)
    monkeypatch.setattr(invoice, "TEMPLATE_FILE", tmp_path / "temp" / "invoice-dynamic.html")
    return tmp_path, year, month


def _stint(home: Path, *args: str) -> str:
    return subprocess.run(
        [str(_BIN), "--home", str(home), "parity", *args],
        capture_output=True,
        text=True,
        check=True,
        encoding="utf-8",
    ).stdout


def test_rust_line_items_match_python(sandbox):
    tmp_path, year, month = sandbox
    from stintcore import invoice

    items, raw = invoice.line_items_for(year, month)
    got = json.loads(_stint(tmp_path, "items", str(year), str(month)))
    assert len(got["items"]) == len(items)
    for g, e in zip(got["items"], items):
        assert g["label"] == e.label
        assert g["narrative"] == e.narrative
        assert g["hours"] == e.hours
        assert g["hours"] * g["rate"] == e.total
    assert got["raw_total"] == raw


def test_rust_staging_matches_python(sandbox):
    tmp_path, year, month = sandbox
    from stintcore import invoice

    items, _ = invoice.line_items_for(year, month)
    py_path = invoice.write_staging(items, year, month, today=date(2026, 7, 10))
    assert _stint(tmp_path, "staging", str(year), str(month)) == py_path.read_text(encoding="utf-8")


def test_rust_html_byte_identical_to_python(sandbox):
    tmp_path, year, month = sandbox
    from stintcore import invoice

    entries = invoice.load_entries(year, month)
    items, _ = invoice.line_items_for(year, month)
    from stintcore import config

    py_html = invoice.build_html(items, entries, year, month, inv_num=FIXED_INV, client_key=config.DEFAULT_CLIENT)
    assert _stint(tmp_path, "html", str(year), str(month), str(FIXED_INV)) == py_html


def test_rust_read_staging_roundtrip(sandbox):
    """Edited staging is picked up identically by both implementations."""
    tmp_path, year, month = sandbox
    from stintcore import invoice

    items, _ = invoice.line_items_for(year, month)
    items[0].hours = 9.75
    items[0].narrative = "edited | with a pipe — and dash"
    invoice.write_staging(items, year, month, today=date(2026, 7, 10))
    py_staged = invoice.read_staging(year, month)
    out = subprocess.run(
        [str(_BIN), "--home", str(tmp_path), "invoice", str(year), str(month), "--json"],
        capture_output=True, text=True, check=True, encoding="utf-8",
    ).stdout
    got = json.loads(out)
    assert got["staged"] is True
    assert [(i["label"], i["hours"], i["narrative"]) for i in got["items"]] == [
        (i.label, i.hours, i.narrative) for i in py_staged
    ]


def test_rust_manual_client_refuses_html(tmp_path):
    (tmp_path / "temp").mkdir()
    shutil.copy(REPO / "samples" / "temp" / "invoice-dynamic.html", tmp_path / "temp" / "invoice-dynamic.html")
    src = sorted((REPO / "samples").glob("stint-????-??.csv"))[-1]
    shutil.copy(src, tmp_path / src.name)
    _, y, m = src.stem.split("-")
    out = subprocess.run(
        [str(_BIN), "--home", str(tmp_path), "invoice", y, m, "--html", "--client", "globex"],
        capture_output=True, text=True, encoding="utf-8",
    )
    assert out.returncode == 1
    assert "not HTML-capable" in out.stderr
    # The number was reserved (counter seeded) but nothing was rendered or committed.
    assert (tmp_path / ".invoice-counter-globex").read_text().strip() == "2001"
    assert not (tmp_path / "temp" / f"invoice-{y}-{m}.html").exists()
