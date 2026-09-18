#!/usr/bin/env python3
"""Generate the Python-side fixtures for the differential corpus.

Run with the tau virtualenv so pydantic is available (or use
`tools/fixtures.sh`, which resolves it):

    ../tau/.venv/bin/python tools/gen_fixtures.py           # write fixtures
    ../tau/.venv/bin/python tools/gen_fixtures.py --check   # verify only
    ../tau/.venv/bin/python tools/gen_fixtures.py --stdout  # print expected JSONL

The Rust differential test (`src/tau_agent/tests/differential.rs`) replays
`tests/fixtures/model_corpus.jsonl` and compares its results against
`tests/fixtures/model_expected.jsonl`, and - when the tau venv is available -
against a live run of this script with `--stdout`.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT.parent / "tau" / "src"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from model_corpus import CASES  # noqa: E402

from pydantic import TypeAdapter, ValidationError  # noqa: E402

from tau_agent.messages import (  # noqa: E402
    AgentMessage,
    AssistantDiagnosticError,
    AssistantMessageDiagnostic,
    ImageContent,
    ResponseTiming,
    TextContent,
    ThinkingContent,
    ToolCall,
    Usage,
    UsageCost,
)
from tau_agent.provider_events import AssistantMessageEvent  # noqa: E402
from tau_agent.tools import AgentToolResult  # noqa: E402

CORPUS_PATH = ROOT / "tests" / "fixtures" / "model_corpus.jsonl"
EXPECTED_PATH = ROOT / "tests" / "fixtures" / "model_expected.jsonl"

ADAPTERS = {
    "agent_message": TypeAdapter(AgentMessage),
    "assistant_message_event": TypeAdapter(AssistantMessageEvent),
    "agent_tool_result": TypeAdapter(AgentToolResult),
    "usage": TypeAdapter(Usage),
    "usage_cost": TypeAdapter(UsageCost),
    "response_timing": TypeAdapter(ResponseTiming),
    "text_content": TypeAdapter(TextContent),
    "thinking_content": TypeAdapter(ThinkingContent),
    "image_content": TypeAdapter(ImageContent),
    "tool_call": TypeAdapter(ToolCall),
    "assistant_diagnostic_error": TypeAdapter(AssistantDiagnosticError),
    "assistant_message_diagnostic": TypeAdapter(AssistantMessageDiagnostic),
}


def compact(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def expected_record(case: dict) -> dict:
    kind = case["kind"]
    if kind not in ADAPTERS:
        raise SystemExit(f"unknown kind {kind!r} in case {case['id']!r}")
    adapter = ADAPTERS[kind]
    try:
        model = adapter.validate_python(case["input"])
    except ValidationError:
        return {"id": case["id"], "accepted": False, "error": "ValidationError"}
    return {"id": case["id"], "accepted": True, "output": adapter.dump_json(model).decode()}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="verify committed fixtures")
    parser.add_argument("--stdout", action="store_true", help="print expected JSONL to stdout")
    args = parser.parse_args()

    corpus_lines = [compact(case) for case in CASES]
    expected_lines = [compact(expected_record(case)) for case in CASES]

    if args.stdout:
        print("\n".join(expected_lines))
        return 0

    if args.check:
        actual_corpus = CORPUS_PATH.read_text(encoding="utf-8").splitlines()
        actual_expected = EXPECTED_PATH.read_text(encoding="utf-8").splitlines()
        if actual_corpus != corpus_lines or actual_expected != expected_lines:
            print("fixtures are out of date; run tools/fixtures.sh", file=sys.stderr)
            return 1
        print(f"fixtures up to date ({len(CASES)} cases)")
        return 0

    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text("\n".join(corpus_lines) + "\n", encoding="utf-8")
    EXPECTED_PATH.write_text("\n".join(expected_lines) + "\n", encoding="utf-8")
    accepted = sum(1 for line in expected_lines if '"accepted":true' in line)
    print(f"wrote {len(CASES)} cases ({accepted} accepted, {len(CASES) - accepted} rejected)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
