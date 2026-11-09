# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "cadquery-ocp-novtk>=8.0.1.1.0",
# ]
# ///
"""Compare experimental PCB/assembly STEP geometry against the LibrePCB oracle."""

import argparse
import json
from pathlib import Path

from appearance_reference import compare as compare_appearance
from OCP.TopAbs import TopAbs_SOLID
from OCP.TopExp import TopExp_Explorer
from reference import measure_shape, read_shape


def solids(path):
    shape = read_shape(path)
    iterator = TopExp_Explorer(shape, TopAbs_SOLID)
    result = []
    while iterator.More():
        result.append(measure_shape(iterator.Current()))
        iterator.Next()
    return result


def bound_error(a, b):
    return max(
        abs(a["bounds"][side][axis] - b["bounds"][side][axis])
        for side in range(2)
        for axis in range(3)
    )


def component_matches(a, b):
    return (
        bound_error(a, b) < 1e-6
        and abs(a["volume"] - b["volume"]) < max(1e-7, abs(b["volume"]) * 1e-7)
        and max(abs(x - y) for x, y in zip(a["center"], b["center"], strict=True))
        < 1e-6
        and a["faces"] == b["faces"]
        and a["valid"]
        and b["valid"]
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("actual", type=Path, help="board prototype output directory")
    parser.add_argument("expected", type=Path, help="official LibrePCB assembly STEP")
    args = parser.parse_args()
    data = json.loads((args.actual / "board.json").read_text())
    expected = solids(args.expected)
    actual = solids(args.actual / "assembly.step")
    assert len(actual) == len(expected) == data["component_solids"] + data["pcb_solids"]
    pcb = solids(args.actual / "pcb.step")
    body_results = []
    for body in pcb:
        # The isolated body export identifies PCB solids without relying on
        # flattened STEP entity order, names or colors.
        original_index = min(
            range(len(expected)), key=lambda i: bound_error(body, expected[i])
        )
        original = expected.pop(original_index)
        placed_index = min(
            range(len(actual)), key=lambda i: bound_error(body, actual[i])
        )
        placed = actual.pop(placed_index)
        assert component_matches(body, placed)
        error = bound_error(body, original)
        volume_error = abs(body["volume"] - original["volume"])
        assert body["valid"] and original["valid"]
        assert error <= max(1e-6, data["contour_tolerance_mm"] * 2)
        assert volume_error <= max(
            1e-7,
            abs(original["volume"]) * 1e-7,
            original["area"] * data["contour_tolerance_mm"] * 2,
        )
        body_results.append(
            {
                "bounds_error_mm": error,
                "volume_error_mm3": volume_error,
                "reference_volume_mm3": original["volume"],
                "analytic_contours": data.get("approximated_contours", 1) == 0,
            }
        )
    component_errors = []
    for solid in actual:
        match = next(
            (
                i
                for i, candidate in enumerate(expected)
                if component_matches(solid, candidate)
            ),
            None,
        )
        assert match is not None, f"unmatched placed component solid: {solid}"
        original = expected.pop(match)
        component_errors.append(bound_error(solid, original))
    assert not expected
    for item in data["meshes"]:
        assert all(
            item["mesh"][key] == 0
            for key in ["boundary_edges", "nonmanifold_edges", "misoriented_edges"]
        ), item["name"]
    result = {
        "passed": True,
        "component_solids_matched": len(component_errors),
        "max_component_bounds_error_mm": max(component_errors, default=0.0),
        "pcb": body_results,
        "component_appearance": compare_appearance(
            args.expected,
            args.actual / "assembly.step",
            [body["bounds"] for body in pcb],
        ),
    }
    (args.actual / "comparison.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
