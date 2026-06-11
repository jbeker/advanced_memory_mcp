# /// script
# requires-python = ">=3.11"
# dependencies = ["mcp>=1.9"]
# ///
"""Capture ground-truth MCP fixtures from a running server.

Writes tools_list.json (full tools/list) and calls.json (a scripted call
sequence covering every tool's success and error paths, including the
temporal fact operations added in v0.4) to the output directory. The server
must be freshly started on an EMPTY data file with a read-write and a
read-only token sharing that file.

The recorded token names are placeholders (fixtok_rw / fixtok_ro /
not_a_real_token); parity_check.py maps real tokens onto them by label
convention when replaying.

Usage:
    uv run benchmarks/capture_fixtures.py --url http://127.0.0.1:8765/mcp \
        --rw-token TOKEN_RW --ro-token TOKEN_RO \
        --out benchmarks/fixtures/rust-v0.4.0
"""

import argparse
import asyncio
import json
from contextlib import asynccontextmanager
from pathlib import Path

from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client

RW = "fixtok_rw"
RO = "fixtok_ro"
BAD = "not_a_real_token"


@asynccontextmanager
async def session(url, token):
    async with streamablehttp_client(url, headers={"Authorization": f"Bearer {token}"}) as (r, w, _):
        async with ClientSession(r, w) as s:
            await s.initialize()
            yield s


def result_to_json(result):
    return {
        "isError": result.isError,
        "content": [
            {"type": c.type, "text": getattr(c, "text", None)} for c in result.content
        ],
        "structuredContent": result.structuredContent,
    }


# (label, token, tool, args). Order matters - later calls depend on earlier
# state. Labels starting with "readonly_" replay with the read-only token,
# "bad_token" with an invalid one; everything else uses the read-write token.
SEQUENCE = [
    # ---- base graph operations (v0.2 lineage) ----
    ("create_entities_success", RW, "create_entities", {"entities": [
        {"name": "Alice", "entityType": "person", "observations": ["Likes coffee"]},
        {"name": "Project X", "entityType": "work project", "observations": []},
        {"name": "Widget", "entityType": "weird gadget thing", "observations": ["Title-case fallback test"]},
    ]}),
    ("create_entities_dedup", RW, "create_entities", {"entities": [
        {"name": "Alice", "entityType": "person", "observations": ["dup, should be skipped"]},
        {"name": "Bob", "entityType": "colleague", "observations": ["New colleague"]},
    ]}),
    ("create_relations_success", RW, "create_relations", {"relations": [
        {"from": "Alice", "to": "Project X", "relationType": "worksOn"},
        {"from": "Bob", "to": "Project X", "relationType": "worksOn"},
        {"from": "Alice", "to": "Bob", "relationType": "manages"},
    ]}),
    ("create_relations_dedup", RW, "create_relations", {"relations": [
        {"from": "Alice", "to": "Project X", "relationType": "worksOn"},
    ]}),
    ("add_observations_success", RW, "add_observations", {"observations": [
        {"entityName": "Alice", "contents": ["Lives in Boston", "Likes coffee"]},
    ]}),
    ("add_observations_missing_entity", RW, "add_observations", {"observations": [
        {"entityName": "Nobody", "contents": ["x"]},
    ]}),
    ("search_nodes", RW, "search_nodes", {"query": "alice"}),
    ("search_nodes_by_observation", RW, "search_nodes", {"query": "boston"}),
    ("search_nodes_no_match", RW, "search_nodes", {"query": "zzz-nothing"}),
    ("open_nodes", RW, "open_nodes", {"names": ["Alice", "Bob"]}),
    ("open_nodes_missing", RW, "open_nodes", {"names": ["Nobody"]}),
    ("read_graph", RW, "read_graph", {}),
    ("rename_entity_success", RW, "rename_entity", {"name": "Widget", "new_name": "Gadget"}),
    ("rename_entity_missing", RW, "rename_entity", {"name": "Nobody", "new_name": "Somebody"}),
    ("rename_entity_collision", RW, "rename_entity", {"name": "Alice", "new_name": "Bob"}),
    ("rename_entity_noop", RW, "rename_entity", {"name": "Alice", "new_name": "Alice"}),
    ("merge_entities_success", RW, "merge_entities", {"source": "Bob", "target": "Alice"}),
    ("merge_entities_missing", RW, "merge_entities", {"source": "Nobody", "target": "Alice"}),
    ("merge_entities_self", RW, "merge_entities", {"source": "Alice", "target": "Alice"}),
    ("normalize_entity_types", RW, "normalize_entity_types", {}),
    ("delete_observations", RW, "delete_observations", {"deletions": [
        {"entityName": "Alice", "observations": ["Likes coffee", "not-present"]},
        {"entityName": "Nobody", "observations": ["ignored silently"]},
    ]}),
    ("delete_relations", RW, "delete_relations", {"relations": [
        {"from": "Alice", "to": "Project X", "relationType": "worksOn"},
    ]}),
    ("delete_entities", RW, "delete_entities", {"entityNames": ["Gadget", "NotThere"]}),

    # ---- temporal facts (v0.4) ----
    ("temporal_setup", RW, "create_entities", {"entities": [
        {"name": "Deploy Plan", "entityType": "project", "observations": []},
    ]}),
    ("add_temporal_fact", RW, "add_observations", {"observations": [
        {"entityName": "Deploy Plan", "contents": [
            {"text": "target: January", "validFrom": "2026-01-10", "source": "1:1 2026-01-10"},
            "untimed legacy-style note",
        ]},
    ]}),
    ("add_superseding_fact", RW, "add_observations", {"observations": [
        {"entityName": "Deploy Plan", "contents": [
            {"text": "target: April", "validFrom": "2026-03-01",
             "source": "1:1 2026-03-01", "supersedes": "target: January"},
        ]},
    ]}),
    ("supersede_missing_target", RW, "add_observations", {"observations": [
        {"entityName": "Deploy Plan", "contents": [
            {"text": "x", "supersedes": "never existed"},
        ]},
    ]}),
    ("supersede_already_closed", RW, "add_observations", {"observations": [
        {"entityName": "Deploy Plan", "contents": [
            {"text": "y", "supersedes": "target: January"},
        ]},
    ]}),
    ("search_facts_default_active", RW, "search_facts", {"query": "target"}),
    ("search_facts_as_of", RW, "search_facts", {"query": "target", "asOf": "2026-02-01"}),
    ("search_facts_window", RW, "search_facts", {"since": "2026-02-01"}),
    ("search_facts_history", RW, "search_facts", {
        "query": "target", "includeSuperseded": True, "order": "asc"}),
    ("search_facts_entity_and_limit", RW, "search_facts", {
        "entityName": "Deploy Plan", "includeSuperseded": True, "limit": 2}),
    ("search_facts_bad_date", RW, "search_facts", {"asOf": "last month"}),
    ("search_facts_bad_order", RW, "search_facts", {"order": "sideways"}),
    ("read_graph_with_facts", RW, "read_graph", {}),

    # ---- auth / permission errors ----
    ("readonly_write_rejected", RO, "create_entities", {"entities": [
        {"name": "Eve", "entityType": "person", "observations": []},
    ]}),
    ("readonly_read_allowed", RO, "search_nodes", {"query": "alice"}),
    ("readonly_search_facts_allowed", RO, "search_facts", {"query": "target"}),
    ("bad_token", BAD, "read_graph", {}),
    ("final_read_graph", RW, "read_graph", {}),
]


async def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--url", required=True)
    parser.add_argument("--rw-token", required=True)
    parser.add_argument("--ro-token", required=True)
    parser.add_argument("--bad-token", default="definitely_not_a_token")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    token_map = {RW: args.rw_token, RO: args.ro_token, BAD: args.bad_token}

    async with session(args.url, args.rw_token) as s:
        tools = await s.list_tools()
        (args.out / "tools_list.json").write_text(
            json.dumps(tools.model_dump(mode="json"), indent=2) + "\n"
        )
        print(f"captured tools/list: {len(tools.tools)} tools")

    calls = []
    for label, token, tool, call_args in SEQUENCE:
        async with session(args.url, token_map[token]) as s:
            result = await s.call_tool(tool, call_args)
        calls.append({
            "label": label, "tool": tool, "args": call_args,
            "result": result_to_json(result),
        })
        print(f"  {label}: ok")

    (args.out / "calls.json").write_text(json.dumps(calls, indent=2) + "\n")
    print(f"wrote {args.out}/calls.json")


if __name__ == "__main__":
    asyncio.run(main())
