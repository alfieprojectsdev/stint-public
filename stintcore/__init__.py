"""stintcore — shared domain core for the stint time tracker.

M0 scope: the billing/duration engine and config, ported from the bash `stint.sh`
script so a single Python source of truth drives both the (future) TUI and the
CLI. The bash script's observable output is the contract; see tests/test_parity.py.
"""

__all__ = ["config", "engine"]
