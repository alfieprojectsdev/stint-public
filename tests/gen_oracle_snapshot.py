"""Snapshot the bash oracle (tests/oracle.sh) for the parity grid.

The oracle needs GNU bash + bc + `date -d`, so it only runs on Linux/WSL. This
writes tests/oracle_snapshot.json so test_rust_parity.py can still assert
byte-parity on Windows/macOS checkouts and in CI runners without bc.

Regenerate (in WSL / Linux) whenever the grid in test_parity.py changes:
    uv run python tests/gen_oracle_snapshot.py
"""

from __future__ import annotations

import json
from datetime import timedelta
from pathlib import Path

from test_parity import _BASE, _DURATION_SECONDS, _oracle, _ts

DESCS = [
    "PR #129 review — diff + cross-repo audit (example-org/example-repo)",
    "plain description",
    "has, commas, in it",
    "em — dash narrative — with, comma",
    "T10 scraper impl (example-org/example-repo)",
    "unicode: résumé café Nguyễn",
]
HRS = ["0.25", "0.50", "0.67", "1.07", "6.18", "42.10", "0.10", "100.00"]


def main() -> None:
    start = _ts(_BASE)
    date = _BASE.strftime("%Y-%m-%d")
    snap = {"hrs": {}, "row": {}, "bill": {}}
    for secs in _DURATION_SECONDS:
        end = _ts(_BASE + timedelta(seconds=secs))
        snap["hrs"][str(secs)] = _oracle("hrs", start, end)
        snap["row"][f"{secs}|pr|{DESCS[0]}"] = _oracle("row", date, start, end, "pr", DESCS[0])
    end40 = _ts(_BASE + timedelta(minutes=40))
    for d in DESCS[1:]:
        snap["row"][f"2400|dev|{d}"] = _oracle("row", date, start, end40, "dev", d)
    for h in HRS:
        snap["bill"][h] = _oracle("bill", h)
    out = Path(__file__).parent / "oracle_snapshot.json"
    out.write_text(json.dumps(snap, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {out} ({len(snap['hrs'])} hrs, {len(snap['row'])} rows, {len(snap['bill'])} bills)")


if __name__ == "__main__":
    main()
