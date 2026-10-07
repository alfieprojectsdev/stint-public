"""Optional LLM narrative *wording* polish for invoice line items.

Financial integrity contract — read before touching this file:

  * Grouping, hour summing, and quarter-rounding all happen in `invoice.py` and
    are NEVER done here. This module rewrites only the human-readable narrative
    prose of already-computed `LineItem`s.
  * The model is handed each item's `label` + terse source `narrative` ONLY. It
    never sees `hours`, `rate`, or `total`, so an LLM error, hallucination, or
    outage cannot change a billed amount.
  * The returned `LineItem`s carry the SAME `label`, `hours`, and `rate` as the
    inputs (asserted in tests). Only `.narrative` is replaced, and only when the
    model returned a usable line for that index — otherwise the original prose
    survives untouched.

Opt-in: this runs only behind `stint.sh consolidate --polish`. The default path is
offline and deterministic and imports nothing from `anthropic`.

Model: Claude Haiku 4.5 (`claude-haiku-4-5`) — cheapest tier, $1/$5 per MTok.
No `thinking` / no `effort` param: `effort` is rejected (400) on Haiku 4.5, and a
prose rewrite needs no extended thinking. The request is tiny, so it is a plain
non-streaming `messages.create`.
"""

from __future__ import annotations

import re

from .invoice import LineItem

MODEL = "claude-haiku-4-5"

_SYSTEM = """You rewrite a contractor's terse time-log notes into clean, professional \
invoice line-item descriptions for a client to read.

Rules:
- Rewrite each note as ONE polished sentence or fragment describing the work delivered.
- Preserve every ticket / PR / issue reference and technical noun EXACTLY (e.g. "T218", "PR #88", "issue #34", repo and library names).
- Do NOT invent work, scope, or detail that is not in the source note.
- Do NOT mention time, hours, dates, rates, or money — those live elsewhere on the invoice.
- Keep it concise: a busy client should grasp what was delivered at a glance.

You receive numbered notes, one per line, formatted:
  [<index>] <label>: <source note>
Return ONLY the rewrites, one per line, formatted:
  [<index>] <polished description>
Emit exactly one line per input index, in the same order. No preamble, no blank lines, no commentary."""

# `[0] rewritten text` — tolerant of leading spaces the model might add.
_LINE_RE = re.compile(r"^\s*\[(\d+)\]\s*(.*\S)\s*$")


def _build_prompt(items: list[LineItem]) -> str:
    lines = []
    for i, it in enumerate(items):
        # label + narrative ONLY — never hours/rate/total.
        note = it.narrative.replace("\n", " ").strip()
        lines.append(f"[{i}] {it.label}: {note}")
    return "\n".join(lines)


def _parse_response(text: str) -> dict[int, str]:
    out: dict[int, str] = {}
    for raw in text.splitlines():
        m = _LINE_RE.match(raw)
        if not m:
            continue
        out[int(m.group(1))] = m.group(2).strip()
    return out


def polish_line_items(
    items: list[LineItem],
    *,
    model: str = MODEL,
    client=None,
    max_tokens: int = 2048,
) -> list[LineItem]:
    """Return copies of `items` with `.narrative` reworded by the LLM.

    `label`, `hours`, and `rate` are carried through byte-for-byte. `client` is
    an `anthropic.Anthropic()` (injectable for tests); when None it is
    constructed here, which resolves ANTHROPIC_API_KEY from the environment.
    """
    if not items:
        return items

    if client is None:
        try:
            import anthropic
        except ImportError as e:  # pragma: no cover - env-dependent
            raise RuntimeError(
                "narrative polish needs the anthropic SDK — run `uv add anthropic`"
            ) from e
        client = anthropic.Anthropic()

    response = client.messages.create(
        model=model,
        max_tokens=max_tokens,
        system=_SYSTEM,
        messages=[{"role": "user", "content": _build_prompt(items)}],
    )
    text = "".join(b.text for b in response.content if getattr(b, "type", None) == "text")
    reworded = _parse_response(text)

    result: list[LineItem] = []
    for i, it in enumerate(items):
        narrative = reworded.get(i, it.narrative).strip() or it.narrative
        # Rebuild explicitly so hours/rate can NEVER be sourced from the model.
        result.append(
            LineItem(label=it.label, narrative=narrative, hours=it.hours, rate=it.rate)
        )
    return result
