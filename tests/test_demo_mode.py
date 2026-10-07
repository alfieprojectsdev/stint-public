"""Demo mode: STINT_DEMO=1 redirects to samples/, ships all badge cases, never
touches the real ledger. config.DEMO is read at import, so these tests set the
env and re-import config in a subprocess-free way via importlib.reload.
"""

from __future__ import annotations

import importlib

import pytest


@pytest.fixture
def demo_config(monkeypatch):
    monkeypatch.setenv("STINT_DEMO", "1")
    import stintcore.config as config

    importlib.reload(config)
    yield config
    monkeypatch.delenv("STINT_DEMO", raising=False)
    importlib.reload(config)  # restore real paths for other tests


def test_demo_points_at_samples(demo_config):
    assert demo_config.DEMO is True
    assert demo_config.LOG_DIR.name == "samples"
    assert demo_config.LOG_DIR.is_dir(), "samples/ must be committed"


def test_samples_present_and_parseable(demo_config):
    import stintcore.store as store

    importlib.reload(store)  # pick up the reloaded config.LOG_DIR
    months = store.available_months()
    assert months, "demo dataset must ship at least one month"
    entries = store.read_month(*months[0])
    assert entries, "newest sample month must have entries"


def test_samples_cover_all_badges(demo_config):
    import stintcore.store as store
    from stintcore.tui.app import _entry_badges

    importlib.reload(store)
    seen = set()
    for y, m in store.available_months():
        for e in store.read_month(y, m):
            seen.add(_entry_badges(e).strip())
    assert "✎" in seen, "need a manual entry to demo the ✎ badge"
    assert "+¼" in seen, "need a sub-0.25h entry to demo the +¼ badge"
    assert "" in seen, "need a plain measured entry (no badge)"


def test_default_mode_is_real_paths(monkeypatch):
    """Without the env var, config points at the repo root, not samples/."""
    monkeypatch.delenv("STINT_DEMO", raising=False)
    import stintcore.config as config

    importlib.reload(config)
    assert config.DEMO is False
    assert config.LOG_DIR.name != "samples"
