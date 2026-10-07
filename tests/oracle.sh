#!/usr/bin/env bash
# Parity oracle for stintcore.engine.
#
# Reproduces the exact arithmetic and row formatting of `stint.sh`
# `_stop_timer_file` (stint.sh:52-60, quarter-hour block, row echo) with NO reboot
# cap — the caller is responsible for passing an already-capped end_time. Every
# line below is a copy of the corresponding line in the real script, so a match
# proves the port.
#
# Usage:
#   oracle.sh row  <entry_date> <"start YYYY-MM-DD HH:MM:SS"> <"end ..."> <cat> <desc>
#   oracle.sh hrs  <"start"> <"end">
#   oracle.sh bill <hrs>
set -euo pipefail

RATE=16.00   # mirror stint.sh:6

# stint.sh:52-60
duration_hrs() {
  local start="$1" end="$2" start_epoch end_epoch diff
  start_epoch=$(date -d "$start" +%s)
  end_epoch=$(date -d "$end" +%s)
  diff=$(( end_epoch - start_epoch ))
  printf "%.2f" "$(echo "scale=4; $diff / 3600" | bc)"
}

# stint.sh quarter-hour block: round measured seconds UP to next 900s (0.25h) bucket.
billed_hrs() {
  local start="$1" end="$2" diff quarters
  diff=$(( $(date -d "$end" +%s) - $(date -d "$start" +%s) ))
  (( diff < 0 )) && diff=0
  quarters=$(( (diff + 899) / 900 ))
  (( quarters < 1 )) && quarters=1
  printf "%.2f" "$(echo "scale=4; $quarters * 0.25" | bc)"
}

case "$1" in
  hrs)
    duration_hrs "$2" "$3"
    ;;
  bill)
    # stint.sh:148
    echo "scale=2; $2 * $RATE" | bc
    ;;
  row)
    entry_date="$2"; start_time="$3"; end_time="$4"; category="$5"; description="$6"
    hrs=$(billed_hrs "$start_time" "$end_time")
    # stint.sh:138
    echo "$entry_date,$(echo "$start_time" | cut -d' ' -f2),$(echo "$end_time" | cut -d' ' -f2),$hrs,$category,\"$description\""
    ;;
  *)
    echo "usage: oracle.sh row|hrs|bill ..." >&2
    exit 2
    ;;
esac
