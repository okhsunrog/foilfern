from pathlib import Path
import json
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
subprocess.run(
    [sys.executable, str(Path(__file__).with_name("inspect.py")), str(root)], check=True
)
r = json.loads((root / "case/review.json").read_text())
expected = {
    "VCC": sorted([f"C{i}.1" for i in range(1, 5)] + [f"R{i}.1" for i in range(1, 4)]),
    "GND": [f"C{i}.2" for i in range(1, 5)],
    "SDA": ["R1.2"],
    "SCL": ["R2.2"],
    "IRQ": ["R3.2"],
}
actual = {net: sorted(pins) for net, pins in r["netlist"]["result"]["nets"].items()}
values = {
    c["designator"]: c["value"] for c in r["component_list"]["result"]["components"]
}
expected_values = {
    "C1": "100nf",
    "C2": "100nf",
    "C3": "100nf",
    "C4": "4.7uf",
    "R1": "2.2k",
    "R2": "2.2k",
    "R3": "10k",
}
normalize = lambda s: s.lower().replace(" ", "").replace("µ", "u")
page = next(page for page in r["pages"] if page["name"] == "Main")
orientation = {}
for symbol in page["schematic"]["symbols"]:
    vcc = [p["position"]["y"] for p in symbol["pins"] if p["net"] == "VCC"]
    other = [p["position"]["y"] for p in symbol["pins"] if p["net"] != "VCC"]
    orientation[symbol["name"]] = (
        len(vcc) == 1 and len(other) == 1 and vcc[0] > other[0]
    )


def segments_for(net, pins):
    wanted = set(pins)
    found = []
    for segment in page["schematic"]["net_segments"]:
        attached = {
            line[end]["pin"]
            for line in segment["lines"]
            for end in ["from", "to"]
            if "pin" in line[end]
        }
        if segment["net"] == net and wanted <= attached:
            found.append(segment["uuid"])
    return found


cap_supply = segments_for("VCC", [f"C{i}.1" for i in range(1, 5)])
cap_ground = segments_for("GND", [f"C{i}.2" for i in range(1, 5)])
pullup_supply = segments_for("VCC", [f"R{i}.1" for i in range(1, 4)])
local_rails = bool(
    cap_supply
    and cap_ground
    and pullup_supply
    and set(cap_supply).isdisjoint(pullup_supply)
)
signal_labels = all(
    any(
        segment["net"] == net and segment["labels"]
        for segment in page["schematic"]["net_segments"]
    )
    for net in ["SDA", "SCL", "IRQ"]
)
acceptance = {
    "exact_netlist": actual == expected,
    "values": {
        key: normalize(values.get(key, "")) == val
        for key, val in expected_values.items()
    },
    "component_count": len(values),
    "annotation_overlaps": len(page["review"]["result"]["annotation_overlaps"]),
    "different_net_wire_overlaps": len(page["different_net_wire_overlaps"]),
    "supply_above": orientation,
    "local_rails": local_rails,
    "signal_labels": signal_labels,
    "titles": [note["text"] for note in page["schematic"]["texts"]],
    "actual_values": values,
}
acceptance["accepted"] = (
    acceptance["exact_netlist"]
    and acceptance["component_count"] == 7
    and all(acceptance["values"].values())
    and not acceptance["annotation_overlaps"]
    and not acceptance["different_net_wire_overlaps"]
    and all(orientation.values())
    and local_rails
    and signal_labels
)
(root / "case/acceptance.json").write_text(json.dumps(acceptance, indent=2) + "\n")
print(json.dumps(acceptance))
