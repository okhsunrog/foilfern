"""Read back a saved native schematic and audit visible geometry independently."""

import base64
import itertools
import json
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
case = root / (sys.argv[3] if len(sys.argv) > 3 else "case")
case.mkdir(exist_ok=True)
project_dir = Path(sys.argv[2]) if len(sys.argv) > 2 else case / "project"
project = next(project_dir.glob("*.lpp"))
command = [
    "taskset",
    "-c",
    "0,1",
    "nice",
    "-n",
    "10",
    str(root / "bin/foilfern-mcp"),
    "--workspace",
    str(root / "workspace"),
    "--project",
    str(project),
]


def batch(calls):
    result = subprocess.run(
        command + ["batch", "-"],
        input=json.dumps(calls),
        text=True,
        capture_output=True,
        check=True,
    )
    records = [json.loads(line) for line in result.stdout.splitlines()]
    for record in records:
        if record["response"].get("isError"):
            raise RuntimeError(record)
    return records


readback = batch(
    [
        {"tool": tool}
        for tool in ["component_list", "netlist", "erc_run", "schematic_list"]
    ]
)
report = {item["tool"]: item["response"]["structuredContent"] for item in readback}
pages = report["schematic_list"]["result"]["schematics"]
report["pages"] = []
for page in pages:
    name = page["name"]
    results = batch(
        [
            {"tool": tool, "arguments": arguments}
            for tool, arguments in [
                ("schematic_get", {"schematic": name}),
                ("schematic_review", {"schematic": name}),
                (
                    "render",
                    {
                        "target": "schematic",
                        "schematic": name,
                        "width": 2600,
                        "height": 2000,
                    },
                ),
            ]
        ]
    )
    sheet = results[0]["response"]["structuredContent"]["result"]
    lines = [
        (segment["net"], line)
        for segment in sheet["net_segments"]
        for line in segment["lines"]
    ]
    overlaps = []
    for (net_a, a), (net_b, b) in itertools.combinations(lines, 2):
        if net_a == net_b:
            continue
        a0, a1 = [a[end]["position"] for end in ["from", "to"]]
        b0, b1 = [b[end]["position"] for end in ["from", "to"]]
        for fixed, along in [("x", "y"), ("y", "x")]:
            if (
                max(a0[fixed], a1[fixed], b0[fixed], b1[fixed])
                - min(a0[fixed], a1[fixed], b0[fixed], b1[fixed])
                > 1e-7
            ):
                continue
            low = max(min(a0[along], a1[along]), min(b0[along], b1[along]))
            high = min(max(a0[along], a1[along]), max(b0[along], b1[along]))
            if high - low > 1e-7:
                overlaps.append(
                    {
                        "nets": [net_a, net_b],
                        "lines": [a["uuid"], b["uuid"]],
                        "fixed_axis": fixed,
                        "coordinate": a0[fixed],
                        "overlap_mm": [low, high],
                    }
                )
    page_report = {
        "name": name,
        "schematic": sheet,
        "review": results[1]["response"]["structuredContent"],
        "different_net_wire_overlaps": overlaps,
    }
    report["pages"].append(page_report)
    for content in results[2]["response"].get("content", []):
        if content["type"] == "image":
            (case / (name.replace("/", "_") + ".png")).write_bytes(
                base64.b64decode(content["data"])
            )
    print(
        name, len(page_report["review"]["result"]["annotation_overlaps"]), len(overlaps)
    )
(case / "review.json").write_text(json.dumps(report, indent=2) + "\n")
