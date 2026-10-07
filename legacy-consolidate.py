#!/usr/bin/env python3
import argparse
import csv
import os
import re
import sys
import tempfile
from collections import OrderedDict
from datetime import date, timedelta
from pathlib import Path



LOG_DIR = Path(__file__).parent
COUNTER_FILE = LOG_DIR / '.invoice-counter'
TEMPLATE_FILE = LOG_DIR / 'temp' / 'invoice-dynamic.html'
RATE = 16.00

CAT_LABELS = {
    'pr':       'Pull Request Reviews',
    'async':    'Async Communication',
    'standup':  'Standups',
    'devops':   'DevOps',
    'research': 'Research',
    'dev':      'Development',
    'admin':    'Administrative',
    'docs':     'Documentation',
    'planning': 'Planning',
}


def load_entries(year, month):
    csv_path = LOG_DIR / f'stint-{year}-{month:02d}.csv'
    if not csv_path.exists():
        legacy = LOG_DIR / f'savd-{year}-{month:02d}.csv'
        if legacy.exists():
            csv_path = legacy
    if not csv_path.exists():
        raise FileNotFoundError(f'No log file found: {csv_path}')
    entries = []
    with open(csv_path, newline='', encoding='utf-8') as f:
        for row in csv.DictReader(f):
            try:
                hrs = float(row['duration_hrs'])
            except (ValueError, KeyError):
                hrs = 0.0
            entries.append({
                'date':        row.get('date', '').strip(),
                'start_time':  row.get('start_time', '').strip(),
                'end_time':    row.get('end_time', '').strip(),
                'hrs':         hrs,
                'category':    row.get('category', '').strip(),
                'description': row.get('description', '').strip(),
            })
    return entries


def extract_group_key(description):
    # Find all ticket-like keys by position in string; return the leftmost
    matches = []
    for m in re.finditer(r'PR\s+#(\d+)', description, re.IGNORECASE):
        matches.append((m.start(), f'PR #{m.group(1)}'))
    for m in re.finditer(r'issue\s+#(\d+)', description, re.IGNORECASE):
        matches.append((m.start(), f'issue #{m.group(1)}'))
    for m in re.finditer(r'\bT(\d+)\b', description):
        matches.append((m.start(), f'T{m.group(1)}'))
    if not matches:
        return None
    return min(matches, key=lambda x: x[0])[1]


def group_entries(entries):
    # Ticket-keyed groups precede category-fallback groups
    ticket_groups: OrderedDict = OrderedDict()
    cat_groups: OrderedDict = OrderedDict()
    for entry in entries:
        key = extract_group_key(entry['description'])
        if key:
            ticket_groups.setdefault(('ticket', key), []).append(entry)
        else:
            cat_groups.setdefault(('cat', entry['category']), []).append(entry)
    result = OrderedDict()
    result.update(ticket_groups)
    result.update(cat_groups)
    return result


def round_quarter(hrs):
    return round(hrs * 4) / 4


def _em_dash_phrase(description):
    parts = re.split(r'\s*[—–]\s*', description, maxsplit=1)
    return parts[1].strip() if len(parts) > 1 else None


def build_line_item(label_type, label_key, entries):
    if label_type == 'ticket':
        display_label = label_key
    else:
        display_label = CAT_LABELS.get(label_key, label_key.title())

    sorted_entries = sorted(entries, key=lambda e: len(e['description']), reverse=True)
    narrative = sorted_entries[0]['description']

    em_phrases = []
    for e in sorted_entries[1:]:
        phrase = _em_dash_phrase(e['description'])
        if phrase and phrase not in em_phrases and phrase not in narrative:
            em_phrases.append(phrase)
        if len(em_phrases) >= 2:
            break

    if em_phrases:
        narrative = narrative + ' — ' + ', '.join(em_phrases)

    hours = round_quarter(sum(e['hrs'] for e in entries))
    return {
        'label':     display_label,
        'narrative': narrative,
        'hours':     hours,
        'rate':      RATE,
        'total':     hours * RATE,
    }


def _wrap(text, width=64):
    words = text.split()
    lines = []
    current = ''
    for word in words:
        if not current:
            current = word
        elif len(current) + 1 + len(word) <= width:
            current += ' ' + word
        else:
            lines.append(current)
            current = word
    if current:
        lines.append(current)
    return lines


def staging_path(year, month):
    return LOG_DIR / 'temp' / f'staging-{year}-{month:02d}.txt'


def write_staging(line_items, year, month):
    path = staging_path(year, month)
    lines = [
        f'# stint.sh invoice staging — {year}-{month:02d}',
        f'# Generated: {date.today()}',
        '# Edit label, hours, and narrative. Delete rows to exclude.',
        '# Rate is fixed at $16.00/hr.',
        '#',
        '# label | hours | narrative',
        '#',
    ]
    for item in line_items:
        lines.append(f'{item["label"]} | {item["hours"]:.2f} | {item["narrative"]}')
    path.write_text('\n'.join(lines) + '\n', encoding='utf-8')
    return path


def read_staging(year, month):
    path = staging_path(year, month)
    if not path.exists():
        return None
    items = []
    for raw_line in path.read_text(encoding='utf-8').splitlines():
        line = raw_line.strip()
        if not line or line.startswith('#'):
            continue
        parts = line.split(' | ', 2)
        if len(parts) < 3:
            continue
        label = parts[0].strip()
        narrative = parts[2].strip()
        try:
            hours = float(parts[1].strip())
        except ValueError:
            continue
        items.append({'label': label, 'narrative': narrative, 'hours': hours,
                      'rate': RATE, 'total': hours * RATE})
    return items if items else None


def render_preview(line_items, raw_total, year, month):
    rounded_total = sum(item['total'] for item in line_items)
    delta = rounded_total - raw_total * RATE

    print()
    print('═' * 72)
    print('  Consolidated Invoice Preview')
    print('═' * 72)
    print(f'  {"Label":<22} {"Hrs":>6}  {"Total":>9}')
    print('─' * 72)
    for item in line_items:
        print(f'  {item["label"]:<22} {item["hours"]:>6.2f}  ${item["total"]:>8.2f}')
        for line in _wrap(item['narrative']):
            print(f'    {line}')
        print()
    print('─' * 72)
    print(f'  Raw CSV total : {raw_total:.4f}h  (${raw_total * RATE:.2f})')
    print(f'  Invoice total : {sum(i["hours"] for i in line_items):.2f}h  (${rounded_total:.2f})')
    delta_str = f'+${delta:.2f}' if delta >= 0 else f'-${abs(delta):.2f}'
    print(f'  Delta         : {delta_str}')
    print('═' * 72)

    path = write_staging(line_items, year, month)
    print()
    print(f'  Staging file  : {path.relative_to(LOG_DIR)}')
    print('  Edit it, then run: stint.sh invoice [YYYY MM] --html')
    print()


def next_invoice_number():
    if not COUNTER_FILE.exists():
        COUNTER_FILE.write_text('1002\n')
        return 1002
    return int(COUNTER_FILE.read_text().strip())


def commit_invoice_number(current):
    fd, tmp_path = tempfile.mkstemp(dir=str(COUNTER_FILE.parent))
    try:
        with os.fdopen(fd, 'w') as f:
            f.write(f'{current + 1}\n')
        os.replace(tmp_path, COUNTER_FILE)
    except Exception:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass
        raise


def _last_day_of_month(year, month):
    if month == 12:
        return date(year, 12, 31)
    return date(year, month + 1, 1) - timedelta(days=1)


def _fmt_date(d):
    return d.strftime('%-d %B %Y')


def _html_escape(text):
    return text.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')


def _js_escape(text):
    return text.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n').replace('\r', '')


def render_html(line_items, entries, year, month):
    inv_num = next_invoice_number()

    issue_date = _last_day_of_month(year, month)
    due_date = issue_date + timedelta(days=15)

    real_dates = [e['date'] for e in entries if e['date'] not in ('', 'manual')]
    if real_dates:
        min_d = date.fromisoformat(min(real_dates))
        max_d = date.fromisoformat(max(real_dates))
    else:
        min_d = date(year, month, 1)
        max_d = issue_date
    billing_period = f'Billing Period: {_fmt_date(min_d)} - {_fmt_date(max_d)}'

    out_path = LOG_DIR / 'temp' / f'invoice-{year}-{month:02d}.html'

    if out_path.exists():
        resp = input(f'Overwrite {out_path.name}? [y/N] ').strip().lower()
        if resp != 'y':
            print('Aborted.')
            sys.exit(2)

    template = TEMPLATE_FILE.read_text(encoding='utf-8')

    # Build JS defaultItems array
    js_rows = []
    for item in line_items:
        label_html = f'<strong>{_html_escape(item["label"])}:</strong>'
        desc_html = f'{label_html} {_html_escape(item["narrative"])}'
        js_rows.append(
            f'            {{ desc: "{_js_escape(desc_html)}",'
            f' hours: {item["hours"]:.2f}, rate: {item["rate"]:.2f} }}'
        )
    js_array = '[\n' + ',\n'.join(js_rows) + '\n        ]'

    template = re.sub(
        r'const defaultItems = \[.*?\];',
        f'const defaultItems = {js_array};',
        template,
        flags=re.DOTALL,
    )

    # Unique storage key so this invoice doesn't share localStorage with others
    template = re.sub(
        r"const STORAGE_KEY = '[^']*';",
        f"const STORAGE_KEY = 'stint_invoice_{year}_{month:02d}';",
        template,
        count=1,
    )

    # Replace static header field contents
    template = re.sub(
        r'(<span id="invoice-number"[^>]*>)[^<]*(</span>)',
        rf'\g<1>{inv_num}\g<2>',
        template,
    )
    template = re.sub(
        r'(<span id="issue-date"[^>]*>)[^<]*(</span>)',
        rf'\g<1>{_fmt_date(issue_date)}\g<2>',
        template,
    )
    template = re.sub(
        r'(<span id="due-date"[^>]*>)[^<]*(</span>)',
        rf'\g<1>{_fmt_date(due_date)}\g<2>',
        template,
    )
    template = re.sub(
        r'(<h3 id="project-title"[^>]*>)[^<]*(</h3>)',
        r'\g<1>Full-Stack Software Architecture &amp; IT Development\g<2>',
        template,
    )
    template = re.sub(
        r'(<p id="billing-period"[^>]*>)[^<]*(</p>)',
        rf'\g<1>{billing_period}\g<2>',
        template,
    )

    out_path.write_text(template, encoding='utf-8')
    commit_invoice_number(inv_num)
    print(f'Invoice written: {out_path.resolve()}')


def main():
    parser = argparse.ArgumentParser(description='stint.sh invoice consolidator')
    parser.add_argument('year', nargs='?', type=int, default=None)
    parser.add_argument('month', nargs='?', type=int, default=None)
    parser.add_argument('--mode', choices=['preview', 'html'], default='preview')
    args = parser.parse_args()

    today = date.today()
    year = args.year if args.year is not None else today.year
    month = args.month if args.month is not None else today.month

    try:
        entries = load_entries(year, month)
    except FileNotFoundError as e:
        print(str(e), file=sys.stderr)
        sys.exit(1)

    groups = group_entries(entries)
    line_items = [build_line_item(lt, lk, ents) for (lt, lk), ents in groups.items()]
    raw_total = sum(e['hrs'] for e in entries)

    if args.mode == 'preview':
        render_preview(line_items, raw_total, year, month)
    else:
        staged = read_staging(year, month)
        if staged is not None:
            print(f'Using staged items from temp/staging-{year}-{month:02d}.txt')
            line_items = staged
        render_html(line_items, entries, year, month)


if __name__ == '__main__':
    main()
