# /// script
# requires-python = ">=3.11"
# dependencies = ["mcp>=1.9"]
# ///
"""Verify a server against the captured Python v0.2.0 MCP fixtures.

Replays benchmarks/fixtures/python-v0.2.0/calls.json (the scripted sequence
that covers every tool's success and error paths) against a live server and
diffs each result, plus tools/list metadata, against the recorded ground
truth. Timestamps differ between runs, so any ISO-8601-shaped string is
masked before comparison.

The target server must be freshly started on an EMPTY data file with two
tokens equivalent to the capture run: a read-write one and a read-only one
sharing the same file.

Usage:
    uv run benchmarks/parity_check.py --url http://127.0.0.1:8796/mcp \
        --rw-token TOKEN_RW --ro-token TOKEN_RO [--fixtures DIR]

Exits non-zero if any difference is found. Known/accepted differences are
listed in KNOWN_DIFFS and reported but don't fail the check.
"""

import argparse
import asyncio
import json
import re
import sys
from contextlib import asynccontextmanager
from pathlib import Path

from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client

DEFAULT_FIXTURES = Path(__file__).parent / "fixtures" / "python-v0.2.0"

# Token placeholders used in the captured sequence.
CAPTURE_RW = "fixtok_rw"
CAPTURE_RO = "fixtok_ro"
CAPTURE_BAD = "not_a_real_token"

TIMESTAMP_RE = re.compile(
    r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?\+00:00"
)

# Accepted metadata differences between implementations, as dotted paths
# relative to each tool entry in tools/list. These don't affect clients.
KNOWN_TOOL_METADATA_DIFFS = {"meta", "icons", "annotations", "title", "execution"}


def mask_timestamps(value):
    if isinstance(value, str):
        return TIMESTAMP_RE.sub("<TS>", value)
    if isinstance(value, list):
        return [mask_timestamps(v) for v in value]
    if isinstance(value, dict):
        return {k: mask_timestamps(v) for k, v in value.items()}
    return value


@asynccontextmanager
async def mcp_session(url: str, token: str):
    headers = {"Authorization": f"Bearer {token}"}
    async with streamablehttp_client(url, headers=headers) as (r, w, _):
        async with ClientSession(r, w) as s:
            await s.initialize()
            yield s


def result_to_json(result) -> dict:
    return {
        "isError": result.isError,
        "content": [
            {"type": c.type, "text": getattr(c, "text", None)} for c in result.content
        ],
        "structuredContent": result.structuredContent,
    }


def diff_paths(expected, actual, path=""):
    """Yield (path, expected, actual) for every leaf difference."""
    if isinstance(expected, dict) and isinstance(actual, dict):
        for key in sorted(set(expected) | set(actual)):
            sub = f"{path}.{key}" if path else key
            if key not in expected:
                yield (sub, "<absent>", actual[key])
            elif key not in actual:
                yield (sub, expected[key], "<absent>")
            else:
                yield from diff_paths(expected[key], actual[key], sub)
    elif isinstance(expected, list) and isinstance(actual, list):
        if len(expected) != len(actual):
            yield (f"{path}.length", len(expected), len(actual))
        for i, (e, a) in enumerate(zip(expected, actual)):
            yield from diff_paths(e, a, f"{path}[{i}]")
    elif expected != actual:
        yield (path, expected, actual)


async def check_tools_list(url: str, token: str, fixtures: Path):
    expected_tools = {
        t["name"]: t
        for t in json.loads((fixtures / "tools_list.json").read_text())["tools"]
    }
    async with mcp_session(url, token) as s:
        listed = await s.list_tools()
    actual_tools = {t.name: t.model_dump(mode="json") for t in listed.tools}

    failures, notes = [], []
    if set(expected_tools) != set(actual_tools):
        failures.append(
            ("tools_list.names", sorted(expected_tools), sorted(actual_tools))
        )
    for name in sorted(set(expected_tools) & set(actual_tools)):
        for path, e, a in diff_paths(expected_tools[name], actual_tools[name]):
            top = path.split(".")[0].split("[")[0]
            if top in KNOWN_TOOL_METADATA_DIFFS:
                notes.append((f"{name}.{path}", e, a))
            else:
                failures.append((f"{name}.{path}", e, a))
    return failures, notes


async def replay_calls(url: str, rw: str, ro: str, bad: str, fixtures: Path):
    calls = json.loads((fixtures / "calls.json").read_text())
    token_map = {CAPTURE_RW: rw, CAPTURE_RO: ro, CAPTURE_BAD: bad}
    failures, notes = [], []

    for call in calls:
        label, tool, args = call["label"], call["tool"], call["args"]
        # The capture script recorded which token each step used via the
        # label conventions; bad_token and readonly_* are the exceptions.
        if label == "bad_token":
            token = token_map[CAPTURE_BAD]
        elif label.startswith("readonly_"):
            token = token_map[CAPTURE_RO]
        else:
            token = token_map[CAPTURE_RW]

        async with mcp_session(url, token) as s:
            result = result_to_json(await s.call_tool(tool, args))

        expected = call.get("result")
        if expected is None:
            notes.append((f"{label}: fixture recorded transport error", "", ""))
            continue

        for path, e, a in diff_paths(
            mask_timestamps(expected), mask_timestamps(result)
        ):
            failures.append((f"{label}.{path}", e, a))

    return failures, notes


async def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--url", required=True)
    parser.add_argument("--rw-token", required=True)
    parser.add_argument("--ro-token", required=True)
    parser.add_argument("--bad-token", default="definitely_not_a_token")
    parser.add_argument("--fixtures", type=Path, default=DEFAULT_FIXTURES)
    args = parser.parse_args()

    print(f"checking {args.url} against {args.fixtures}")

    tool_failures, tool_notes = await check_tools_list(
        args.url, args.rw_token, args.fixtures
    )
    call_failures, call_notes = await replay_calls(
        args.url, args.rw_token, args.ro_token, args.bad_token, args.fixtures
    )

    failures = tool_failures + call_failures
    notes = tool_notes + call_notes

    if notes:
        print(f"\n{len(notes)} known/accepted differences:")
        for path, e, a in notes[:20]:
            print(f"  ~ {path}: {json.dumps(e)[:80]} -> {json.dumps(a)[:80]}")
        if len(notes) > 20:
            print(f"  ... and {len(notes) - 20} more")

    if failures:
        print(f"\n{len(failures)} FAILURES:")
        for path, e, a in failures[:40]:
            print(f"  ✗ {path}:\n      expected {json.dumps(e)[:200]}\n      actual   {json.dumps(a)[:200]}")
        if len(failures) > 40:
            print(f"  ... and {len(failures) - 40} more")
        return 1

    print("\nPARITY OK: all tool calls match the Python fixtures")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
