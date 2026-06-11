# /// script
# requires-python = ">=3.11"
# dependencies = ["mcp>=1.9"]
# ///
"""Black-box performance benchmark for any Advanced Memory MCP implementation.

Talks only the public MCP streamable-HTTP protocol, so it can compare this
server against a reimplementation in any language. All setup and teardown
happens through public tool calls; the data file is never touched directly.

WARNING: point this at a throwaway token/data file. It creates and deletes
thousands of entities.

Usage:
    uv run benchmarks/mcp_bench.py --url http://127.0.0.1:8765/mcp --token TOKEN
    uv run benchmarks/mcp_bench.py ... --sizes 100,1000,10000 --json results.json

Phases per graph size:
  1. Seed N entities (5 observations each) and N relations via create_entities /
     create_relations.
  2. Measure median latency of representative read and write tools.
Then a concurrency check fires parallel single-entity creates over separate
connections and counts survivors, detecting lost updates.
Finally all benchmark entities are deleted unless --keep-data is given.
"""

import argparse
import asyncio
import json
import statistics
import sys
import time
from contextlib import asynccontextmanager
from datetime import datetime, timezone

from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client

ENTITY_PREFIX = "mcpbench"
SEED_BATCH = 500


@asynccontextmanager
async def mcp_session(url: str, token: str):
    headers = {"Authorization": f"Bearer {token}"}
    async with streamablehttp_client(url, headers=headers) as (read, write, _):
        async with ClientSession(read, write) as session:
            await session.initialize()
            yield session


def root_error(exc: BaseException) -> str:
    """Drill through ExceptionGroup wrappers to the first leaf exception."""
    while isinstance(exc, BaseExceptionGroup) and exc.exceptions:
        exc = exc.exceptions[0]
    return f"{type(exc).__name__}: {exc}"


def tool_payload(result) -> dict:
    """Extract the JSON payload from a tool result, raising on tool errors."""
    if result.isError:
        text = result.content[0].text if result.content else "unknown error"
        raise RuntimeError(f"tool call failed: {text}")
    return json.loads(result.content[0].text)


async def timed_call(session, tool, args, repeats):
    samples = []
    for _ in range(repeats):
        t0 = time.perf_counter()
        result = await session.call_tool(tool, args)
        samples.append((time.perf_counter() - t0) * 1000)
        tool_payload(result)  # surface errors instead of timing failures
    return {
        "median_ms": round(statistics.median(samples), 3),
        "min_ms": round(min(samples), 3),
        "max_ms": round(max(samples), 3),
    }


def seed_entity(i: int) -> dict:
    return {
        "name": f"{ENTITY_PREFIX}-e-{i}",
        "entityType": "Person" if i % 2 else "Project",
        "observations": [
            f"Observation {j} about entity {i}: moderately sized text payload"
            for j in range(5)
        ],
    }


async def seed(session, current: int, target: int) -> float:
    """Grow the seeded graph from `current` to `target` entities. Returns seconds."""
    t0 = time.perf_counter()
    for start in range(current, target, SEED_BATCH):
        batch = [seed_entity(i) for i in range(start, min(start + SEED_BATCH, target))]
        tool_payload(await session.call_tool("create_entities", {"entities": batch}))
    relations = [
        {
            "from": f"{ENTITY_PREFIX}-e-{i}",
            "to": f"{ENTITY_PREFIX}-e-{(i + 1) % target}",
            "relationType": "knows",
        }
        for i in range(current, target)
    ]
    for start in range(0, len(relations), SEED_BATCH):
        tool_payload(await session.call_tool(
            "create_relations", {"relations": relations[start:start + SEED_BATCH]}
        ))
    return time.perf_counter() - t0


async def latency_suite(session, size: int, repeats: int) -> dict:
    mid = f"{ENTITY_PREFIX}-e-{size // 2}"
    suite = {
        "search_nodes": ("search_nodes", {"query": f"{ENTITY_PREFIX}-e-7"}),
        "open_nodes": ("open_nodes", {"names": [mid]}),
        "read_graph": ("read_graph", {}),
        "create_entities_1": None,  # handled below: needs unique names
        "add_observations_1": None,
        "create_relations_1": None,
    }
    results = {}
    for label in ("search_nodes", "open_nodes", "read_graph"):
        tool, args = suite[label]
        results[label] = await timed_call(session, tool, args, repeats)

    # Write ops need unique arguments per call so dedup doesn't short-circuit
    # the save path; build a uniqueness counter from the size context.
    counter = [0]

    async def timed_unique(tool, make_args):
        samples = []
        for _ in range(repeats):
            counter[0] += 1
            args = make_args(counter[0])
            t0 = time.perf_counter()
            result = await session.call_tool(tool, args)
            samples.append((time.perf_counter() - t0) * 1000)
            tool_payload(result)
        return {
            "median_ms": round(statistics.median(samples), 3),
            "min_ms": round(min(samples), 3),
            "max_ms": round(max(samples), 3),
        }

    results["create_entities_1"] = await timed_unique(
        "create_entities",
        lambda c: {"entities": [{
            "name": f"{ENTITY_PREFIX}-w-{size}-{c}",
            "entityType": "Person",
            "observations": ["write benchmark"],
        }]},
    )
    results["add_observations_1"] = await timed_unique(
        "add_observations",
        lambda c: {"observations": [{
            "entityName": mid,
            "contents": [f"bench observation {size}-{c}"],
        }]},
    )
    results["create_relations_1"] = await timed_unique(
        "create_relations",
        lambda c: {"relations": [{
            "from": mid,
            "to": f"{ENTITY_PREFIX}-e-1",
            "relationType": f"benchRel{size}x{c}",
        }]},
    )
    return results


async def race_check(url: str, token: str, n_concurrent: int) -> dict:
    """Fire concurrent single-entity creates on separate connections, count survivors."""

    errors = []

    async def create_one(i):
        # Failures count as findings, not benchmark crashes: a server that
        # errors under concurrent writes is part of what we're measuring.
        try:
            async with mcp_session(url, token) as s:
                tool_payload(await s.call_tool("create_entities", {"entities": [{
                    "name": f"{ENTITY_PREFIX}-race-{i}",
                    "entityType": "Person",
                    "observations": ["race check"],
                }]}))
            return True
        except Exception as exc:
            errors.append(root_error(exc))
            return False

    t0 = time.perf_counter()
    acks = await asyncio.gather(*(create_one(i) for i in range(n_concurrent)))
    elapsed = time.perf_counter() - t0
    acked = sum(1 for ok in acks if ok)

    # The survivor count itself can fail if concurrent writes corrupted the
    # store badly enough that reads no longer work; report that, don't crash.
    survivors = None
    store_readable = True
    try:
        async with mcp_session(url, token) as s:
            data = tool_payload(await s.call_tool(
                "search_nodes", {"query": f"{ENTITY_PREFIX}-race-"}
            ))
        survivors = sum(
            1 for e in data["entities"] if e["name"].startswith(f"{ENTITY_PREFIX}-race-")
        )
    except Exception as exc:
        store_readable = False
        errors.append(f"post-race read failed: {root_error(exc)}")
    return {
        "sent": n_concurrent,
        "acknowledged": acked,
        "call_errors": len(errors),
        "error_samples": sorted(set(e.splitlines()[0] for e in errors))[:3],
        "store_readable": store_readable,
        "survived": survivors,
        "lost_after_ack": (acked - survivors) if survivors is not None else None,
        "elapsed_s": round(elapsed, 2),
    }


async def cleanup(session) -> int:
    """Delete every entity this benchmark created, in batches. Returns count."""
    data = tool_payload(await session.call_tool(
        "search_nodes", {"query": f"{ENTITY_PREFIX}-"}
    ))
    names = [
        e["name"] for e in data["entities"] if e["name"].startswith(f"{ENTITY_PREFIX}-")
    ]
    for start in range(0, len(names), SEED_BATCH):
        tool_payload(await session.call_tool(
            "delete_entities", {"entityNames": names[start:start + SEED_BATCH]}
        ))
    return len(names)


def print_table(size_results: list[dict]) -> None:
    if not size_results:
        return
    op_names = list(size_results[0]["tools"].keys())
    header = f"{'entities':>9} {'seed s':>7}"
    for op in op_names:
        header += f" {op:>19}"
    print(header)
    for row in size_results:
        line = f"{row['size']:>9} {row['seed_seconds']:>7.1f}"
        for op in op_names:
            line += f" {row['tools'][op]['median_ms']:>19.2f}"
        print(line)
    print("(tool columns: median ms)")


async def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--url", required=True, help="MCP endpoint, e.g. http://127.0.0.1:8765/mcp")
    parser.add_argument("--token", required=True, help="Bearer token (read-write, throwaway data file)")
    parser.add_argument("--sizes", default="100,1000,10000",
                        help="Comma-separated graph sizes to benchmark (default: 100,1000,10000)")
    parser.add_argument("--repeats", type=int, default=10, help="Calls per measurement (default: 10)")
    parser.add_argument("--race-concurrency", type=int, default=40,
                        help="Parallel creates in the race check (default: 40, 0 to skip)")
    parser.add_argument("--json", dest="json_out", help="Write machine-readable results to this file")
    parser.add_argument("--keep-data", action="store_true", help="Skip cleanup of benchmark entities")
    args = parser.parse_args()
    sizes = sorted(int(s) for s in args.sizes.split(","))

    report = {
        "url": args.url,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "repeats": args.repeats,
        "sizes": [],
        "race": None,
    }

    async with mcp_session(args.url, args.token) as session:
        tools = await session.list_tools()
        print(f"connected to {args.url}; {len(tools.tools)} tools advertised")
        report["tool_count"] = len(tools.tools)

        seeded = 0
        for size in sizes:
            print(f"\nseeding {seeded} -> {size} entities...")
            seed_seconds = await seed(session, seeded, size)
            seeded = size
            print(f"seeded in {seed_seconds:.1f}s; measuring ({args.repeats} calls per tool)...")
            tools_result = await latency_suite(session, size, args.repeats)
            report["sizes"].append({
                "size": size,
                "seed_seconds": round(seed_seconds, 1),
                "tools": tools_result,
            })

    if args.race_concurrency > 0:
        print(f"\nrace check: {args.race_concurrency} concurrent single-entity creates...")
        report["race"] = await race_check(args.url, args.token, args.race_concurrency)
        r = report["race"]
        clean = r["call_errors"] == 0 and r["lost_after_ack"] == 0 and r["store_readable"]
        print(
            f"sent {r['sent']} in {r['elapsed_s']}s: "
            f"{r['acknowledged']} acknowledged, {r['call_errors']} call errors, "
            f"{r['survived']} survived"
        )
        if not r["store_readable"]:
            print("STORE CORRUPTED: reads fail after concurrent writes")
        elif r["lost_after_ack"]:
            print(f"DATA LOSS: {r['lost_after_ack']} acknowledged entities missing afterwards")
        if r["error_samples"]:
            print("sample errors: " + "; ".join(r["error_samples"]))
        if clean:
            print("no loss or errors detected")

    if not args.keep_data:
        try:
            async with mcp_session(args.url, args.token) as session:
                deleted = await cleanup(session)
            print(f"\ncleanup: deleted {deleted} benchmark entities")
        except Exception as exc:
            print(f"\ncleanup FAILED (store likely corrupted): {root_error(exc)}")

    print()
    print_table(report["sizes"])

    if args.json_out:
        with open(args.json_out, "w") as f:
            json.dump(report, f, indent=2)
        print(f"\nresults written to {args.json_out}")

    race = report["race"]
    race_clean = race is None or (
        race["call_errors"] == 0 and race["lost_after_ack"] == 0 and race["store_readable"]
    )
    return 0 if race_clean else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
