import asyncio
import base64
import hashlib
import json
import os
import sys
from collections import defaultdict
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

CASE = Path(sys.argv[1]).resolve()
SCENARIO = next(
    s
    for s in json.loads(
        (Path(__file__).resolve().parent / "scenarios.json").read_text()
    )
    if s["id"] == sys.argv[2]
)


async def inspect():
    params = StdioServerParameters(
        command=str(Path(os.environ["FOILFERN_CHECK_BINARY"])),
        args=["--project", str(CASE / "project/Schematic_Trial.lpp")],
    )
    results = {}
    async with (
        stdio_client(params) as (read, write),
        ClientSession(read, write) as session,
    ):
        await session.initialize()
        for name, args in [
            ("server_info", {}),
            ("component_list", {}),
            ("netlist", {}),
            ("schematic_get", {}),
            ("schematic_review", {}),
            ("erc_run", {}),
            ("render", {"target": "schematic", "width": 2400, "height": 1800}),
        ]:
            response = await session.call_tool(name, args)
            raw = response.model_dump(mode="json", by_alias=True, exclude_none=True)
            for content in raw.get("content", []):
                if content.get("type") == "image":
                    (CASE / "schematic.png").write_bytes(
                        base64.b64decode(content["data"])
                    )
            structured = raw.get("structuredContent", raw.get("structured_content", {}))
            (CASE / f"check-{name}.json").write_text(json.dumps(structured, indent=2))
            results[name] = structured["result"]
    netlist = results["netlist"]
    expected = {name: sorted(pins) for name, pins in SCENARIO["expected"].items()}
    actual = {name: sorted(pins) for name, pins in netlist["nets"].items()}
    components = results["component_list"]["components"]
    values = {component["designator"]: component["value"] for component in components}
    values_match = all(
        values.get(name) == component["value"]
        for name, component in SCENARIO["components"].items()
        if component["type"] != "test_pad"
    )
    sheet = results["schematic_get"]
    by_net = defaultdict(list)
    for segment in sheet["net_segments"]:
        pins = set()
        for line in segment["lines"]:
            for endpoint in [line["from"], line["to"]]:
                if "pin" in endpoint:
                    pins.add(endpoint["pin"])
        by_net[segment["net"]].append(sorted(pins))
    physical_wiring = all(
        sorted({pin for group in by_net.get(net, []) for pin in group}) == pins
        and (
            len(by_net[net]) >= 2
            if SCENARIO["id"] == "dual-rc" and net == "GND"
            else len(by_net[net]) == 1
        )
        for net, pins in expected.items()
    )
    layout = {
        "symbols": sorted(
            [
                {
                    "component": s["component"],
                    "position": s["position"],
                    "rotation": s["rotation"],
                    "mirrored": s["mirrored"],
                }
                for s in sheet["symbols"]
            ],
            key=lambda s: s["component"],
        ),
        "texts": [
            {key: value for key, value in text.items() if key != "uuid"}
            for text in sheet["texts"]
        ],
    }
    report = {
        "scenario": SCENARIO["id"],
        "netlist_matches": expected == actual,
        "expected_netlist": expected,
        "actual_netlist": actual,
        "component_values_match": values_match,
        "component_count": len(components),
        "expected_component_count": len(SCENARIO["components"]),
        "erc": {k: results["erc_run"][k] for k in ["errors", "warnings", "hints"]},
        "annotation_overlaps": len(results["schematic_review"]["annotation_overlaps"]),
        "physical_wiring_matches": physical_wiring,
        "physical_wire_groups": dict(by_net),
        "layout": layout,
        "layout_sha256": hashlib.sha256(
            json.dumps(layout, sort_keys=True).encode()
        ).hexdigest(),
        "visual_review": "pending",
    }
    (CASE / "checks.json").write_text(json.dumps(report, indent=2))
    print(
        json.dumps(
            {
                k: v
                for k, v in report.items()
                if k not in ["layout", "expected_netlist", "actual_netlist"]
            },
            indent=2,
        )
    )
    accepted = (
        expected == actual
        and values_match
        and len(components) == len(SCENARIO["components"])
        and physical_wiring
        and not report["annotation_overlaps"]
        and not any(report["erc"].values())
    )
    if not accepted:
        raise SystemExit("Native schematic checks failed. See checks.json.")


asyncio.run(inspect())
