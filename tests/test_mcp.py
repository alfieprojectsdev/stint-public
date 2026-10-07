"""`stint mcp`: drive the stdio MCP server with raw JSON-RPC and check the
tool surface, the JSON shapes (same as the CLI's --json), the on-disk effects,
and that failures come back as tool results (isError) rather than protocol
errors. Skipped when no binary is built, like the other Rust harnesses.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parent.parent


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


class Mcp:
    def __init__(self, home: Path):
        self.p = subprocess.Popen(
            [str(_BIN), "--home", str(home), "mcp"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding="utf-8",
        )
        self.n = 0
        init = self.request("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "pytest", "version": "0"},
        })
        assert init["serverInfo"]["name"] == "stint"
        assert "tools" in init["capabilities"]
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _send(self, msg: dict) -> None:
        self.p.stdin.write(json.dumps(msg) + "\n")
        self.p.stdin.flush()

    def request(self, method: str, params: dict) -> dict:
        self.n += 1
        self._send({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params})
        line = self.p.stdout.readline()
        assert line, f"server closed stdout; stderr: {self.p.stderr.read()[:500]}"
        msg = json.loads(line)
        assert "error" not in msg, msg
        return msg["result"]

    def call(self, name: str, args: dict | None = None) -> tuple[bool, dict | list | str]:
        r = self.request("tools/call", {"name": name, "arguments": args or {}})
        text = r["content"][0]["text"]
        try:
            payload = json.loads(text)
        except ValueError:
            payload = text
        return bool(r.get("isError", False)), payload

    def close(self) -> int:
        self.p.stdin.close()
        return self.p.wait(timeout=10)


@pytest.fixture
def mcp(tmp_path):
    (tmp_path / "temp").mkdir()
    shutil.copy(REPO / "samples" / "temp" / "invoice-dynamic.html", tmp_path / "temp" / "invoice-dynamic.html")
    m = Mcp(tmp_path)
    yield tmp_path, m
    assert m.close() == 0


def test_tool_surface(mcp):
    _home, m = mcp
    tools = {t["name"]: t for t in m.request("tools/list", {})["tools"]}
    assert set(tools) == {
        "start_timer", "stop_timer", "list_running", "log_month", "add_entry",
        "summary", "consolidate_preview", "render_invoice", "ledger_info",
    }
    assert tools["start_timer"]["inputSchema"]["required"] == ["description"]
    assert set(tools["add_entry"]["inputSchema"]["required"]) == {"description", "category", "hours"}
    assert "quarter hour" in tools["stop_timer"]["description"]


def test_timer_lifecycle_writes_the_ledger(mcp):
    home, m = mcp
    err, t = m.call("start_timer", {"description": "T77 mcp probe", "category": "pr"})
    assert not err and t["timer_id"] == 1 and (home / ".timers" / "1.timer").exists()

    err, running = m.call("list_running")
    assert not err and len(running) == 1 and running[0]["live_amount"] == "0.00"

    err, r = m.call("stop_timer")
    assert not err
    assert r["hrs"] == "0.25" and r["billable"] == "4.00" and r["timer_id"] == 1
    assert not (home / ".timers" / "1.timer").exists()
    csv = (home / f"stint-{r['entry_date'][:7]}.csv").read_text(encoding="utf-8")
    assert r["csv_row"] in csv

    err, msg = m.call("stop_timer")
    assert err and "no timers running" in msg


def test_ambiguous_stop_lists_ids(mcp):
    _home, m = mcp
    m.call("start_timer", {"description": "a"})
    m.call("start_timer", {"description": "b"})
    err, msg = m.call("stop_timer")
    assert err and "[1, 2]" in msg
    err, r = m.call("stop_timer", {"id": 2})
    assert not err and r["timer_id"] == 2


def test_validation_is_the_core_validation(mcp):
    _home, m = mcp
    err, msg = m.call("start_timer", {"description": "x", "category": "a,b"})
    assert err and "comma" in msg
    err, msg = m.call("add_entry", {"description": "x", "category": "docs", "hours": "abc"})
    assert err and "decimal" in msg
    err, msg = m.call("add_entry", {"description": "x", "category": "docs", "hours": "1.5", "date": "09/01/2026"})
    assert err and "bad date" in msg
    err, msg = m.call("add_entry", {"description": "   ", "category": "docs", "hours": "1.5"})
    assert err and "description is required" in msg
    err, msg = m.call("log_month", {"year": 2026, "month": 13})
    assert err and "month must be 1-12" in msg
    err, msg = m.call("render_invoice", {"year": 2026, "month": 0})
    assert err and "month must be 1-12" in msg
    err, e = m.call("add_entry", {"description": "typo cat", "category": "Dev", "hours": "1", "date": "2026-09-05"})
    assert not err and "not a known category" in e["note"]


def test_manual_entry_summary_and_log(mcp):
    _home, m = mcp
    err, e = m.call("add_entry", {"description": "backdated", "category": "docs", "hours": "1.5", "date": "2026-09-01"})
    assert not err and e["start_time"] == "manual" and e["hrs"] == "1.5"
    err, log = m.call("log_month", {"year": 2026, "month": 9})
    assert not err and log["entries"][0]["description"] == "backdated"
    assert log["totals"] == {"hours": "1.5", "amount": "24.00"}
    err, s = m.call("summary")
    assert not err and set(s) == {"today", "week", "month"}


def test_consolidate_and_render(mcp):
    home, m = mcp
    m.call("add_entry", {"description": "T5 build the thing", "category": "dev", "hours": "2.2", "date": "2026-09-02"})
    m.call("add_entry", {"description": "standup", "category": "standup", "hours": "0.5", "date": "2026-09-03"})
    err, c = m.call("consolidate_preview", {"year": 2026, "month": 9})
    assert not err and c["staged"] is False
    assert [i["label"] for i in c["items"]] == ["T5", "Standups"]
    assert c["items"][0]["hours"] == 2.25  # quarter-rounded
    staging = home / "temp" / "staging-2026-09.txt"
    assert staging.exists()

    # A hand edit survives a second preview unless overwrite is requested.
    staging.write_text(staging.read_text(encoding="utf-8").replace("T5 | 2.25", "T5 | 9.75"), encoding="utf-8")
    err, c2 = m.call("consolidate_preview", {"year": 2026, "month": 9})
    assert not err and c2["staged"] is True and c2["items"][0]["hours"] == 9.75 and "overwrite" in c2["note"]
    err, c3 = m.call("consolidate_preview", {"year": 2026, "month": 9, "overwrite": True})
    assert not err and c3["staged"] is False and c3["items"][0]["hours"] == 2.25

    err, r = m.call("render_invoice", {"year": 2026, "month": 9})
    assert not err and r["invoice_number"] == 1001
    assert (home / "temp" / "invoice-2026-09.html").exists()
    assert (home / ".invoice-counter").read_text().strip() == "1002"

    err, k = m.call("render_invoice", {"year": 2026, "month": 9, "client": "globex"})
    assert not err and k["html"] is None and k["reserved_invoice_number"] == 2001
    assert (home / ".invoice-counter-globex").read_text().strip() == "2002"

    err, msg = m.call("render_invoice", {"client": "nope"})
    assert err and "unknown client" in msg


def test_ledger_info(mcp):
    home, m = mcp
    err, info = m.call("ledger_info")
    assert not err and info["demo"] is False
    assert info["default_client"] == "acme" and [c["key"] for c in info["clients"]] == ["acme", "globex"]
    assert Path(info["log_dir"]) == home
