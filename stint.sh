#!/usr/bin/env bash
# stint.sh - Time tracker and invoice generator for freelance work
# Usage: stint.sh start|stop [id]|stop-all|status|watch [id]|work|log|invoice|add

# ── Config ────────────────────────────────────────────────────────────────────
RATE=16.00
# STINT_LOG_DIR is the current name; SAVD_LOG_DIR is the pre-rename spelling.
LOG_DIR="${STINT_LOG_DIR:-${SAVD_LOG_DIR:-$HOME/repos/stint}}"
TIMERS_DIR="$LOG_DIR/.timers"
CURRENT_YEAR=$(date +%Y)
CURRENT_MONTH=$(date +%m)

# ── Categories ────────────────────────────────────────────────────────────────
# pr       - Claude Code PR/code reviews
# async    - Async chat sessions with the client
# standup  - Meetings / standups
# devops   - GCP / GitHub Actions / CI-CD work
# research - Researching tricky dev/ops issues
# dev      - General software development
# admin    - Administrative / onboarding
# docs     - Documentation & architectural review
# planning - Project planning & strategy

# ── Helpers ───────────────────────────────────────────────────────────────────
mkdir -p "$LOG_DIR" "$TIMERS_DIR"

# Migrate legacy single-timer file to numbered slot
_migrate_legacy_timer() {
  local legacy="$LOG_DIR/.current_timer"
  if [[ -f "$legacy" ]]; then
    local id=1
    while [[ -f "$TIMERS_DIR/$id.timer" ]]; do (( id++ )); done
    { echo "id=$id"; cat "$legacy"; } > "$TIMERS_DIR/$id.timer"
    rm "$legacy"
    echo "ℹ  Migrated legacy timer to slot #$id"
  fi
}
_migrate_legacy_timer

# The month's CSV. A month already stored under the legacy `savd-` prefix (the
# tool was named after a client once) keeps that file; anything new is
# `stint-YYYY-MM.csv`. The Python and Rust cores resolve it the same way.
csv_file() {
  local year="${1:-$CURRENT_YEAR}"
  local month="${2:-$CURRENT_MONTH}"
  local current="$LOG_DIR/stint-${year}-${month}.csv"
  local legacy="$LOG_DIR/savd-${year}-${month}.csv"
  if [[ ! -f "$current" && -f "$legacy" ]]; then
    echo "$legacy"
  else
    echo "$current"
  fi
}

ensure_header() {
  local file="$1"
  if [[ ! -f "$file" ]]; then
    echo "date,start_time,end_time,duration_hrs,category,description" > "$file"
  fi
}

duration_hrs() {
  local start="$1"
  local end="$2"
  local start_epoch end_epoch diff
  start_epoch=$(date -d "$start" +%s 2>/dev/null)
  end_epoch=$(date -d "$end" +%s 2>/dev/null)
  diff=$(( end_epoch - start_epoch ))
  printf "%.2f" "$(echo "scale=4; $diff / 3600" | bc)"
}

next_timer_id() {
  local i=1
  while [[ -f "$TIMERS_DIR/$i.timer" ]]; do (( i++ )); done
  echo "$i"
}

running_timer_files() {
  # Prints one path per line; produces no output if none exist
  local f
  for f in "$TIMERS_DIR"/*.timer; do
    [[ -f "$f" ]] && echo "$f"
  done
}

_read_timer() {
  local file="$1" key="$2"
  grep "^${key}=" "$file" | cut -d= -f2-
}

# Earliest system boot strictly after the given epoch, printed as epoch.
# Empty if none / `last` unavailable. stint timers are *files*, not processes —
# they survive reboots and keep accruing elapsed time, so a timer left running
# across a reboot inflates to now-start. This lets _stop_timer_file cap the end
# at the first boot after start (when the machine went down = work stopped).
_first_boot_after() {
  local start_epoch="$1" best="" b b_epoch
  while read -r b; do
    [[ -n "$b" ]] || continue
    b_epoch=$(date -d "$b" +%s 2>/dev/null) || continue
    if (( b_epoch > start_epoch )) && { [[ -z "$best" ]] || (( b_epoch < best )); }; then
      best="$b_epoch"
    fi
  done < <(last --time-format iso reboot 2>/dev/null | awk '$1=="reboot"{print $5}')
  echo "$best"
}

_stop_timer_file() {
  local timer_file="$1"
  local id entry_date start_time category description
  id=$(_read_timer "$timer_file" id)
  entry_date=$(_read_timer "$timer_file" date)
  start_time=$(_read_timer "$timer_file" start)
  category=$(_read_timer "$timer_file" category)
  description=$(_read_timer "$timer_file" description)

  local end_time
  end_time=$(date "+%Y-%m-%d %H:%M:%S")

  # Reboot guard: if a boot happened between start and now, the timer file merely
  # survived the reboot — cap the end at that first boot so we don't bill downtime.
  local start_epoch end_epoch boot_epoch capped=0
  start_epoch=$(date -d "$start_time" +%s 2>/dev/null)
  end_epoch=$(date -d "$end_time" +%s 2>/dev/null)
  boot_epoch=$(_first_boot_after "$start_epoch")
  if [[ -n "$boot_epoch" ]] && (( boot_epoch < end_epoch )); then
    end_time=$(date -d "@$boot_epoch" "+%Y-%m-%d %H:%M:%S")
    capped=1
  fi

  local hrs measured diff_sec quarters rounded=0
  hrs=$(duration_hrs "$start_time" "$end_time")
  measured=$hrs

  # Quarter-hour round-up: billed time is the measured session time rounded UP to
  # the next 15-minute (0.25h) mark. Floor + ceiling in one rule:
  #   * 0 < t <= 15m      -> 0.25h   (a short session always bills a full quarter)
  #   * 15m < t <= 30m    -> 0.50h,  30m < t <= 45m -> 0.75h, and so on.
  # A session landing exactly on a quarter-hour boundary is unchanged. Manual
  # `stint.sh add` entries are exempt (the hours are typed explicitly).
  diff_sec=$(( $(date -d "$end_time" +%s) - $(date -d "$start_time" +%s) ))
  (( diff_sec < 0 )) && diff_sec=0
  quarters=$(( (diff_sec + 899) / 900 ))   # ceil to 900s (15m) buckets
  (( quarters < 1 )) && quarters=1
  hrs=$(printf "%.2f" "$(echo "scale=4; $quarters * 0.25" | bc)")
  (( $(echo "$hrs != $measured" | bc -l) )) && rounded=1

  local year month file
  year=$(echo "$entry_date" | cut -d- -f1)
  month=$(echo "$entry_date" | cut -d- -f2)
  file=$(csv_file "$year" "$month")
  ensure_header "$file"

  echo "$entry_date,$(echo "$start_time" | cut -d' ' -f2),$(echo "$end_time" | cut -d' ' -f2),$hrs,$category,\"$description\"" >> "$file"
  rm "$timer_file"

  if (( capped )); then
    echo "⚠  Timer #${id} spanned a reboot — end capped at first boot ($end_time)."
    echo "   (stint timers are files and survive reboots; edit the CSV if the real stop differs.)"
  fi
  echo "■  Timer #${id} stopped."
  echo "   [$category] $description"
  echo "   $start_time → $end_time"
  echo "   Duration: ${hrs}h  |  Billable: \$$(echo "scale=2; $hrs * $RATE" | bc)"
  if (( rounded )); then
    # Explain the adjustment inline so any caller — including a Claude Code
    # session in another repo that can't see this project's conventions — knows
    # the bump is the intended billing rule, not a rounding/floor bug.
    echo "   ⚡ quarter-hour rule: measured ${measured}h rounded UP to next 0.25h mark = ${hrs}h billed."
    echo "      Intended billing convention (15-min billing granularity), NOT a bug — do not \"fix\" it."
  fi
}

# ── Commands ──────────────────────────────────────────────────────────────────

cmd_start() {
  local description="$1"
  local category="${2:-dev}"

  # Order-tolerant: canonical is `stint.sh start "description" [category]`, but a
  # common slip is `stint.sh start <category> "description"`. If arg1 is a known
  # category and arg2 is not, the args were reversed — swap them so the CSV
  # columns (category, description) are always written correctly.
  local known_categories=" pr async standup devops research dev admin docs planning "
  if [[ -n "$2" ]] \
     && [[ "$known_categories" == *" $1 "* ]] \
     && [[ "$known_categories" != *" $2 "* ]]; then
    description="$2"
    category="$1"
  fi

  if [[ -z "$description" ]]; then
    echo "Usage: stint.sh start \"description\" [category]"
    echo "Categories: pr, async, standup, devops, research, dev, admin, docs, planning"
    exit 1
  fi

  local id
  id=$(next_timer_id)
  local now
  now=$(date "+%Y-%m-%d %H:%M:%S")
  {
    echo "id=$id"
    echo "date=$(date +%Y-%m-%d)"
    echo "start=$now"
    echo "category=$category"
    echo "description=$description"
  } > "$TIMERS_DIR/$id.timer"

  echo "▶  Timer #${id} started: [$category] $description"
  echo "   Started at: $now"
}

cmd_stop() {
  local target="$1"
  local timer_file

  if [[ -n "$target" ]]; then
    timer_file="$TIMERS_DIR/$target.timer"
    if [[ ! -f "$timer_file" ]]; then
      echo "No timer #$target found."
      exit 1
    fi
  else
    local files=()
    while IFS= read -r f; do files+=("$f"); done < <(running_timer_files)
    local count="${#files[@]}"
    if [[ "$count" -eq 0 ]]; then
      echo "No timers running. Use 'stint.sh start' first."
      exit 1
    elif [[ "$count" -eq 1 ]]; then
      timer_file="${files[0]}"
    else
      echo "Multiple timers running — specify an ID: stint.sh stop <id>"
      echo ""
      cmd_status
      exit 1
    fi
  fi

  _stop_timer_file "$timer_file"
}

cmd_stop_all() {
  local files=()
  while IFS= read -r f; do files+=("$f"); done < <(running_timer_files)
  if [[ "${#files[@]}" -eq 0 ]]; then
    echo "No timers running."
    exit 0
  fi
  for f in "${files[@]}"; do
    _stop_timer_file "$f"
    echo ""
  done
}

cmd_status() {
  local files=()
  while IFS= read -r f; do files+=("$f"); done < <(running_timer_files)
  if [[ "${#files[@]}" -eq 0 ]]; then
    echo "No timers running."
    return
  fi

  local now
  now=$(date +%s)
  for timer_file in "${files[@]}"; do
    local id start_time description category start_epoch elapsed_secs elapsed_min
    id=$(_read_timer "$timer_file" id)
    start_time=$(_read_timer "$timer_file" start)
    category=$(_read_timer "$timer_file" category)
    description=$(_read_timer "$timer_file" description)
    start_epoch=$(date -d "$start_time" +%s)
    elapsed_secs=$(( now - start_epoch ))
    elapsed_min=$(( elapsed_secs / 60 ))
    echo "⏱  #${id} Running: [$category] $description"
    echo "   Started: $start_time"
    echo "   Elapsed: ${elapsed_min}m"
  done
}

cmd_log() {
  local year="${1:-$CURRENT_YEAR}"
  local month="${2:-$CURRENT_MONTH}"
  local file
  file=$(csv_file "$year" "$month")

  if [[ ! -f "$file" ]]; then
    echo "No log found for $year-$month."
    exit 0
  fi

  echo ""
  echo "═══════════════════════════════════════════════════"
  echo "  Time Log — $year-$month"
  echo "═══════════════════════════════════════════════════"
  printf "%-12s %-8s %-8s %-6s %-10s %s\n" "Date" "Start" "End" "Hrs" "Category" "Description"
  echo "───────────────────────────────────────────────────"

  local total_hrs=0
  while IFS=, read -r date start end hrs cat desc; do
    [[ "$date" == "date" ]] && continue
    printf "%-12s %-8s %-8s %-6s %-10s %s\n" "$date" "$start" "$end" "$hrs" "$cat" "${desc//\"/}"
    total_hrs=$(echo "scale=4; $total_hrs + $hrs" | bc)
  done < "$file"

  local total_hrs_fmt total_bill
  total_hrs_fmt=$(printf "%.2f" "$total_hrs")
  total_bill=$(echo "scale=2; $total_hrs * $RATE" | bc)

  echo "───────────────────────────────────────────────────"
  echo "  Total hours : ${total_hrs_fmt}h"
  echo "  Rate        : \$$RATE/hr"
  echo "  Total due   : \$$total_bill USD"
  echo "═══════════════════════════════════════════════════"
  echo ""
}

cmd_consolidate() {
  # Default path is the untouched legacy consolidator. `--polish` opts into the
  # stintcore path, which rewords narratives via Claude Haiku (wording only —
  # hours/totals are computed first and never sent to the model).
  local polish=0
  local pos_args=()
  for arg in "$@"; do
    case "$arg" in
      --polish) polish=1 ;;
      *)        pos_args+=("$arg") ;;
    esac
  done
  local year="${pos_args[0]:-$CURRENT_YEAR}"
  local month="${pos_args[1]:-$CURRENT_MONTH}"

  if (( polish )); then
    exec uv run --project "$LOG_DIR" python -m stintcore.consolidate "$year" "$month" --polish
  fi
  python3 "$LOG_DIR/legacy-consolidate.py" "$year" "$month" --mode preview
}

cmd_invoice() {
  local year month html_flag=0
  local pos_args=()
  for arg in "$@"; do
    case "$arg" in
      --html) html_flag=1 ;;
      *)      pos_args+=("$arg") ;;
    esac
  done
  year="${pos_args[0]:-$CURRENT_YEAR}"
  month="${pos_args[1]:-$CURRENT_MONTH}"

  if (( html_flag )); then
    python3 "$LOG_DIR/legacy-consolidate.py" "$year" "$month" --mode html
    return
  fi

  local file
  file=$(csv_file "$year" "$month")

  if [[ ! -f "$file" ]]; then
    echo "No log found for $year-$month."
    exit 0
  fi

  declare -A cat_hrs

  while IFS=, read -r date start end hrs cat desc; do
    [[ "$date" == "date" ]] && continue
    cat_hrs[$cat]=$(echo "scale=4; ${cat_hrs[$cat]:-0} + $hrs" | bc)
  done < "$file"

  local total_hrs=0
  echo ""
  echo "═══════════════════════════════════════════════════════"
  echo "  Invoice Summary — $year-$month"
  echo "  Rate: \$$RATE/hr"
  echo "═══════════════════════════════════════════════════════"
  printf "%-12s %-8s %-8s %s\n" "Category" "Hours" "Total" "Notes"
  echo "───────────────────────────────────────────────────────"

  for cat in "${!cat_hrs[@]}"; do
    local hrs total
    hrs=$(printf "%.2f" "${cat_hrs[$cat]}")
    total=$(echo "scale=2; ${cat_hrs[$cat]} * $RATE" | bc)
    printf "%-12s %-8s \$%-7s\n" "$cat" "${hrs}h" "$total"
    total_hrs=$(echo "scale=4; $total_hrs + ${cat_hrs[$cat]}" | bc)
  done

  local total_hrs_fmt total_bill
  total_hrs_fmt=$(printf "%.2f" "$total_hrs")
  total_bill=$(echo "scale=2; $total_hrs * $RATE" | bc)

  echo "───────────────────────────────────────────────────────"
  echo "  Total hours : ${total_hrs_fmt}h"
  echo "  Total due   : \$$total_bill USD"
  echo "═══════════════════════════════════════════════════════"
  echo ""
  echo "CSV source: $file"
  echo ""
}

parse_duration() {
  local input="$1"
  if [[ "$input" =~ ^([0-9]+)h([0-9]+)m$ ]]; then
    echo $(( BASH_REMATCH[1] * 3600 + BASH_REMATCH[2] * 60 ))
  elif [[ "$input" =~ ^([0-9]+)h$ ]]; then
    echo $(( BASH_REMATCH[1] * 3600 ))
  elif [[ "$input" =~ ^([0-9]+)m$ ]]; then
    echo $(( BASH_REMATCH[1] * 60 ))
  elif [[ "$input" =~ ^([0-9]+)s$ ]]; then
    echo "${BASH_REMATCH[1]}"
  else
    echo "Invalid duration '$input' — use 90m, 2h, 1h30m, 45s" >&2
    return 1
  fi
}

cmd_watch() {
  local target="$1"
  local timer_file

  if [[ -n "$target" ]]; then
    timer_file="$TIMERS_DIR/$target.timer"
    if [[ ! -f "$timer_file" ]]; then
      echo "No timer #$target found."
      exit 1
    fi
  else
    local files=()
    while IFS= read -r f; do files+=("$f"); done < <(running_timer_files)
    local count="${#files[@]}"
    if [[ "$count" -eq 0 ]]; then
      echo "No timers running. Use 'stint.sh start' first."
      exit 1
    elif [[ "$count" -eq 1 ]]; then
      timer_file="${files[0]}"
    else
      echo "Multiple timers running — specify an ID: stint.sh watch <id>"
      echo ""
      cmd_status
      exit 1
    fi
  fi

  local id start_time description category start_epoch elapsed
  id=$(_read_timer "$timer_file" id)
  start_time=$(_read_timer "$timer_file" start)
  description=$(_read_timer "$timer_file" description)
  category=$(_read_timer "$timer_file" category)
  start_epoch=$(date -d "$start_time" +%s)
  elapsed=$(( $(date +%s) - start_epoch ))

  echo "⏱  Watching #${id}: [$category] $description"
  echo "   Started: $start_time"
  sleep 1

  "$HOME/scripts/timer.sh" --offset "$elapsed"
}

cmd_work() {
  local duration="$1"
  local description="${2:-}"
  local category="${3:-dev}"

  if [[ -z "$duration" ]]; then
    echo "Usage: stint.sh work <duration> [\"description\"] [category]"
    echo "       stint.sh work 90m \"T6 implementation\" dev"
    exit 1
  fi

  local secs
  secs=$(parse_duration "$duration") || exit 1

  local id timer_file label now
  id=$(next_timer_id)
  timer_file="$TIMERS_DIR/$id.timer"
  label="${description:-work block (${duration})}"
  now=$(date "+%Y-%m-%d %H:%M:%S")
  {
    echo "id=$id"
    echo "date=$(date +%Y-%m-%d)"
    echo "start=$now"
    echo "category=$category"
    echo "description=$label"
  } > "$timer_file"
  echo "▶  Timer #${id} started: [$category] $label"

  local beep_args=()
  (( secs > 300 )) && beep_args=(--beep $(( secs - 300 )))

  "$HOME/scripts/timer.sh" --countdown "$secs" "${beep_args[@]}"

  echo ""
  if [[ -z "$description" ]]; then
    read -rp "Description (enter to keep current): " entered
    if [[ -n "$entered" ]]; then
      local tmp
      tmp=$(mktemp)
      grep -v "^description=" "$timer_file" > "$tmp"
      echo "description=$entered" >> "$tmp"
      mv "$tmp" "$timer_file"
    fi
  fi

  read -rp "Stop and log timer #${id}? [Y/n] " confirm
  if [[ ! "$confirm" =~ ^[Nn]$ ]]; then
    _stop_timer_file "$timer_file"
  else
    echo "Timer #${id} still running. Use 'stint.sh stop $id' when done."
  fi
}

cmd_add() {
  local description="$1"
  local category="$2"
  local hrs="$3"
  local entry_date="${4:-$(date +%Y-%m-%d)}"

  if [[ -z "$description" || -z "$category" || -z "$hrs" ]]; then
    echo "Usage: stint.sh add \"description\" category hours [YYYY-MM-DD]"
    exit 1
  fi

  local year month
  year=$(echo "$entry_date" | cut -d- -f1)
  month=$(echo "$entry_date" | cut -d- -f2)
  local file
  file=$(csv_file "$year" "$month")
  ensure_header "$file"

  echo "$entry_date,manual,manual,$hrs,$category,\"$description\"" >> "$file"
  local total
  total=$(echo "scale=2; $hrs * $RATE" | bc)
  echo "✓ Added: [$category] $description — ${hrs}h (\$$total)"
}

# ── Main ──────────────────────────────────────────────────────────────────────
case "$1" in
  start)    cmd_start "$2" "$3" ;;
  stop)     cmd_stop "$2" ;;
  stop-all) cmd_stop_all ;;
  status)   cmd_status ;;
  watch)    cmd_watch "$2" ;;
  work)     cmd_work "$2" "$3" "$4" ;;
  tui)
    # `stint.sh tui --demo` (or `stint.sh demo`) launches against the synthetic samples/
    # dataset instead of the real ledger — safe to screenshot / show off.
    [[ "$2" == "--demo" ]] && export STINT_DEMO=1
    exec uv run --project "$LOG_DIR" python -m stintcore.tui ;;
  demo)        STINT_DEMO=1 exec uv run --project "$LOG_DIR" python -m stintcore.tui ;;
  log)         cmd_log "$2" "$3" ;;
  consolidate) cmd_consolidate "${@:2}" ;;
  invoice)     cmd_invoice "${@:2}" ;;
  add)         cmd_add "$2" "$3" "$4" "$5" ;;
  *)
    echo "stint.sh — time tracker  (\$${RATE}/hr)"
    echo ""
    echo "  start       Begin a new timer (multiple can run concurrently)"
    echo "                stint.sh start \"T10 URL scraper implementation\" dev"
    echo "                stint.sh start \"PR #26 review\" pr"
    echo ""
    echo "  stop        Stop a timer and log it to the monthly CSV"
    echo "                stint.sh stop          # only timer running"
    echo "                stint.sh stop 2        # timer #2 when multiple are running"
    echo "              Billing rule: measured time is rounded UP to the next"
    echo "              15-minute (0.25h) mark. So 3m → 0.25h, 17m → 0.50h."
    echo "              Intended 15-min granularity, not a rounding bug."
    echo ""
    echo "  stop-all    Stop and log every running timer at once"
    echo "                stint.sh stop-all"
    echo ""
    echo "  status      Show all running timers with elapsed time"
    echo "                stint.sh status"
    echo ""
    echo "  watch       Live TUI countdown/elapsed view for a running timer"
    echo "                stint.sh watch         # only timer running"
    echo "                stint.sh watch 2       # timer #2"
    echo ""
    echo "  work        Countdown block that auto-logs when done"
    echo "                stint.sh work 90m \"T6 Peishuo ingestion\" dev"
    echo "                stint.sh work 1h30m    # prompts for description at end"
    echo ""
    echo "  tui         Full-screen dashboard: live timers, quick-start, totals,"
    echo "              and a filterable month log (read + track)"
    echo "                stint.sh tui"
    echo "                stint.sh tui --demo   # synthetic sample data (safe to show off)"
    echo ""
    echo "  demo        Launch the TUI against the synthetic samples/ dataset"
    echo "                stint.sh demo         # alias for: stint.sh tui --demo"
    echo ""
    echo "  log         Formatted table of entries for a month"
    echo "                stint.sh log           # current month"
    echo "                stint.sh log 2026 04   # April 2026"
    echo ""
    echo "  consolidate Group entries by ticket/category, round to 0.25h, show preview"
    echo "              Writes temp/staging-YYYY-MM.txt for you to edit before invoicing"
    echo "                stint.sh consolidate           # current month"
    echo "                stint.sh consolidate 2026 04"
    echo "                stint.sh consolidate 2026 04 --polish  # reword narratives via Claude Haiku"
    echo "              --polish: wording only — hours/totals computed first, never sent to the model"
    echo ""
    echo "  invoice     Category summary (default) or full HTML invoice (--html)"
    echo "              --html reads temp/staging-YYYY-MM.txt if present"
    echo "                stint.sh invoice               # current month, terminal summary"
    echo "                stint.sh invoice 2026 04"
    echo "                stint.sh invoice --html        # writes temp/invoice-YYYY-MM.html"
    echo "                stint.sh invoice 2026 04 --html"
    echo ""
    echo "  add         Manually append an entry (use for backdated or inferred hours)"
    echo "                stint.sh add \"T6 Peishuo ingestion — PR #17\" dev 6.18 2026-04-23"
    echo "                stint.sh add \"Weekly sync\" standup 0.5 2026-04-17"
    echo ""
    echo "Monthly invoice flow:"
    echo "  stint.sh consolidate 2026 04          # review groupings; staging file written"
    echo "  \$EDITOR temp/staging-2026-04.txt  # fix labels, hours, narratives"
    echo "  stint.sh invoice 2026 04 --html       # HTML ready to open and Print to PDF"
    echo ""
    echo "Duration formats: 90m  2h  1h30m  45s"
    echo "Categories: pr  async  standup  devops  research  dev  admin  docs  planning"
    ;;
esac
