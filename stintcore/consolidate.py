"""CLI: consolidate a month into staging line items, optionally LLM-polished.

    python -m stintcore.consolidate [YYYY MM] [--polish]

Default (no flag) is offline and deterministic — it groups + rounds via
`invoice.line_items_for` and writes `temp/staging-YYYY-MM.txt`. `--polish`
additionally rewords the narratives through Claude Haiku (`polish.py`); hours and
totals are computed BEFORE the model runs and are never sent to it, so the flag
can only change wording, never a billed amount. The staging file stays the
human-in-the-loop review point — nothing is invoiced here.
"""

from __future__ import annotations

import argparse
from datetime import date

from . import config, invoice


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="stint.sh consolidate")
    p.add_argument("year", nargs="?", type=int)
    p.add_argument("month", nargs="?", type=int)
    p.add_argument(
        "--polish",
        action="store_true",
        help="reword narratives via Claude Haiku (wording only; hours/totals untouched)",
    )
    p.add_argument("--client", default=config.DEFAULT_CLIENT)
    args = p.parse_args(argv)

    today = date.today()
    year = args.year or today.year
    month = args.month or today.month

    items, raw_total = invoice.line_items_for(year, month)

    if args.polish:
        from . import polish

        before = [(it.label, it.hours, it.rate) for it in items]
        try:
            items = polish.polish_line_items(items)
        except Exception as e:  # network/auth/SDK — never block the deterministic path
            print(f"⚠ polish skipped ({type(e).__name__}: {e}); writing unpolished staging.")
        else:
            after = [(it.label, it.hours, it.rate) for it in items]
            # Hard invariant: the LLM may only touch wording.
            assert before == after, "polish altered label/hours/rate — refusing to write"
            print("✓ narratives polished via Claude Haiku (hours/totals unchanged).")

    path = invoice.write_staging(items, year, month, today)

    total = sum(it.hours for it in items)
    print(f"\nStaging written: {path}")
    print(f"  {len(items)} line item(s), {total:.2f} billable hrs "
          f"(raw {raw_total:.2f} hrs).")
    print("  Edit it, then: stint.sh invoice %d %02d --html" % (year, month))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
