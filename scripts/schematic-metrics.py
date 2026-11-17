#!/usr/bin/env python3
"""Measure the schematic layout of saved projects and compare them.

Runs `foilfern-mcp batch` with `schematic_review` on every page and prints the
page metrics. With two projects (for example before/after a relayout or two
agent runs of the same prompt), prints the change of each value per page.

    scripts/schematic-metrics.py --bin target/release/foilfern-mcp \\
        --workspace WS before/Board.lpp after/Board.lpp [--json out.json]

Read-only: the tool never saves the project.
"""

import argparse
import json
from pathlib import Path
import subprocess
import sys

COLUMNS = [
    ("issues", ("readability_issues",)),
    ("defects", ("defect_count",)),
    ("symbols", ("symbols",)),
    ("width", ("content", "width_mm")),
    ("height", ("content", "height_mm")),
    ("fill", ("symbol_fill",)),
    ("wire_mm", ("wires", "total_length_mm")),
    ("longest", ("wires", "longest_mm")),
    ("bends", ("wires", "bends")),
    ("cross", ("wires", "crossings")),
    ("labels", ("nets", "labels")),
    ("split", ("nets", "split_nets")),
    ("span_mm", ("nets", "pin_span_mm")),
    ("unwired", ("nets", "unwired_pins")),
    ("offgrid", ("off_grid",)),
]


def batch(args, project, calls):
    command = [str(args.bin)]
    if args.workspace:
        command += ["--workspace", str(args.workspace)]
    command += ["--project", str(project), "batch", "-"]
    result = subprocess.run(
        command, input=json.dumps(calls), text=True, capture_output=True
    )
    if result.returncode != 0:
        sys.exit(f"{project}: foilfern-mcp failed:\n{result.stderr}")
    records = [json.loads(line) for line in result.stdout.splitlines()]
    for record in records:
        if record["response"].get("isError"):
            sys.exit(f"{project}: {record['tool']} failed: {record['response']}")
    return [record["response"]["structuredContent"]["result"] for record in records]


def measure(args, project):
    pages = batch(args, project, [{"tool": "schematic_list"}])[0]["schematics"]
    names = [page["name"] for page in pages]
    reviews = batch(
        args,
        project,
        [
            {"tool": "schematic_review", "arguments": {"schematic": page["uuid"]}}
            for page in pages
        ],
    )
    return {name: review for name, review in zip(names, reviews)}


def value(metrics, path):
    for key in path:
        if metrics is None:
            return None
        metrics = metrics.get(key)
    if isinstance(metrics, dict):  # off_grid
        return sum(metrics.values())
    return metrics


def fmt(number):
    if number is None:
        return "-"
    if isinstance(number, float):
        return f"{number:.3f}".rstrip("0").rstrip(".")
    return str(number)


def table(rows):
    widths = [max(len(row[i]) for row in rows) for i in range(len(rows[0]))]
    for row in rows:
        print("  ".join(cell.rjust(width) for cell, width in zip(row, widths)))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("projects", nargs="+", type=Path, help=".lpp files")
    parser.add_argument("--bin", type=Path, default=Path("target/release/foilfern-mcp"))
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--json", type=Path, help="write the full reviews here")
    args = parser.parse_args()
    if len(args.projects) > 2:
        parser.error("pass one project, or two to compare")
    measured = [measure(args, project) for project in args.projects]
    header = ["page"] + [name for name, _ in COLUMNS]
    for project, pages in zip(args.projects, measured):
        print(f"# {project}")
        rows = [header]
        for name, review in pages.items():
            rows.append(
                [name] + [fmt(value(review["metrics"], path)) for _, path in COLUMNS]
            )
        table(rows)
        for name, review in pages.items():
            defects = review["metrics"]["defects"]
            for kind, items in defects.items():
                for item in items:
                    print(f"  {name}: {kind}: {json.dumps(item)}")
        print()
    if len(measured) == 2:
        before, after = measured
        print("# change (second - first)")
        rows = [header]
        for name in [page for page in before if page in after]:
            row = [name]
            for _, path in COLUMNS:
                a = value(before[name]["metrics"], path)
                b = value(after[name]["metrics"], path)
                row.append("-" if a is None or b is None else fmt(b - a))
            rows.append(row)
        table(rows)
        for name in set(before) ^ set(after):
            print(f"  page {name!r} exists in only one project")
    if args.json:
        args.json.write_text(
            json.dumps(
                {str(p): pages for p, pages in zip(args.projects, measured)}, indent=2
            )
            + "\n"
        )


if __name__ == "__main__":
    main()
