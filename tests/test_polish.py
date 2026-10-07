"""`--polish` narrative rewrite: the LLM may only touch wording.

The Anthropic client is mocked — no network, no key needed. The guardrail these
tests protect: hours, rate, total, and label of every line item are byte-identical
before and after polishing; only `.narrative` changes.
"""

from __future__ import annotations

from stintcore import polish
from stintcore.invoice import RATE, LineItem


class _Block:
    type = "text"

    def __init__(self, text):
        self.text = text


class _Resp:
    def __init__(self, text):
        self.content = [_Block(text)]


class FakeClient:
    """Stands in for anthropic.Anthropic(). Records the prompt, returns canned text."""

    def __init__(self, reply):
        self._reply = reply
        self.seen_prompt = None
        self.seen_kwargs = None
        self.messages = self

    def create(self, **kwargs):
        self.seen_kwargs = kwargs
        self.seen_prompt = kwargs["messages"][0]["content"]
        return _Resp(self._reply)


def _items():
    return [
        LineItem(label="T218", narrative="scraper impl", hours=6.25),
        LineItem(label="PR #88", narrative="review + fixes", hours=1.5),
        LineItem(label="Development", narrative="misc dev", hours=0.25),
    ]


def test_hours_rate_label_untouched():
    reply = (
        "[0] Implemented the T218 URL scraper.\n"
        "[1] Reviewed PR #88 and applied requested fixes.\n"
        "[2] General development tasks.\n"
    )
    src = _items()
    out = polish.polish_line_items(src, client=FakeClient(reply))

    assert [it.label for it in out] == [it.label for it in src]
    assert [it.hours for it in out] == [it.hours for it in src]
    assert [it.rate for it in out] == [RATE, RATE, RATE]
    assert [it.total for it in out] == [it.total for it in src]
    # Wording DID change.
    assert out[0].narrative == "Implemented the T218 URL scraper."
    assert out[1].narrative == "Reviewed PR #88 and applied requested fixes."


def test_prompt_never_contains_numbers():
    """The model must not see hours, rate, or total — only label + narrative."""
    client = FakeClient("[0] x\n[1] y\n[2] z\n")
    polish.polish_line_items(_items(), client=client)
    prompt = client.seen_prompt
    for forbidden in ("6.25", "1.50", "1.5", "0.25", "16.00", "100.0", "$"):
        assert forbidden not in prompt, f"prompt leaked {forbidden!r}: {prompt!r}"
    # But labels + source narratives ARE present.
    assert "T218" in prompt and "scraper impl" in prompt


def test_no_effort_or_thinking_param():
    """effort is rejected (400) on Haiku 4.5; thinking is unnecessary here."""
    client = FakeClient("[0] a\n[1] b\n[2] c\n")
    polish.polish_line_items(_items(), client=client)
    assert "effort" not in client.seen_kwargs
    assert "thinking" not in client.seen_kwargs
    assert client.seen_kwargs["model"] == "claude-haiku-4-5"


def test_missing_or_garbled_lines_keep_original():
    # Model only returned index 0; 1 and 2 must fall back to source narrative.
    src = _items()
    out = polish.polish_line_items(src, client=FakeClient("[0] Rewrote it.\n"))
    assert out[0].narrative == "Rewrote it."
    assert out[1].narrative == "review + fixes"
    assert out[2].narrative == "misc dev"


def test_empty_rewrite_falls_back():
    # A blank rewrite must not wipe the narrative.
    src = _items()[:1]
    out = polish.polish_line_items(src, client=FakeClient("[0]   \n"))
    assert out[0].narrative == "scraper impl"


def test_empty_items_no_call():
    # No items → returns immediately, never constructs a client or calls the API.
    assert polish.polish_line_items([]) == []


def test_parse_response_tolerant():
    text = "preamble junk\n  [1]  spaced out  \n[0] first\nnot a line\n"
    parsed = polish._parse_response(text)
    assert parsed == {1: "spaced out", 0: "first"}
