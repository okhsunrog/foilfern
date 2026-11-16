"""Prepare a frozen small task from an empty native project with its part library."""

import argparse
import hashlib
import json
import shutil
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--root", type=Path, required=True)
parser.add_argument("--binary", type=Path, required=True)
parser.add_argument("--seed", type=Path, required=True)
parser.add_argument("--commit", required=True)
parser.add_argument(
    "--scenario", choices=["divider", "rc-filter", "dual-rc"], required=True
)
parser.add_argument("--case", required=True)
args = parser.parse_args()
root = args.root.resolve()
binary = args.binary.resolve()
case = root / args.case
case.mkdir(parents=True)
shutil.copytree(args.seed, case / "project")
projects = list((case / "project").glob("*.lpp"))
if len(projects) != 1:
    raise ValueError(
        "The seed must contain exactly one .lpp project, an empty Main sheet and the required project library"
    )
projects[0].rename(case / "project/Schematic_Trial.lpp")
here = Path(__file__).resolve().parent
scenario = next(
    s
    for s in json.loads((here / "scenarios.json").read_text())
    if s["id"] == args.scenario
)
parts = json.loads((here / "parts.json").read_text())
catalog = json.loads(subprocess.check_output([str(binary), "tools"]))
catalog_path = case / "tools.json"
catalog_path.write_text(json.dumps(catalog, indent=2))
fixture = {
    "scenario": scenario,
    "parts": parts,
    "source_access": False,
    "binary_condition": {
        "commit": args.commit,
        "binary": str(binary),
        "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "profile": "release",
        "catalog_tools": len(catalog["tools"]),
    },
}
(case / "fixture.json").write_text(json.dumps(fixture, indent=2))
prompt = (
    (here / "prompt-prefix.txt")
    .read_text()
    .replace("@BINARY@", str(binary))
    .replace("@CATALOG@", str(catalog_path))
)
prompt += "PARTS\n" + json.dumps(parts, indent=2)
prompt += "\n\nTASK\n" + scenario["request"]
prompt += "\n\nThe exact designators and net names above are required. Save the finished project. Review its rendered image and ensure labels, values and wires are readable and do not overlap. Do not stop after merely creating a netlist. You may use up to 35 tool turns. Report what you checked and anything left unresolved in at most 150 words.\n\n"
prompt += (
    "SERVER STARTING INSTRUCTIONS\n"
    + catalog["instructions"]
    + "\n\nFULL TOOL CATALOG\n"
)
prompt += "\n\n".join(
    f"## {tool['name']}\n\n{tool.get('description', '')}" for tool in catalog["tools"]
)
(case / "prompt.txt").write_text(prompt)
print(case)
