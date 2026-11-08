# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "cadquery-ocp-novtk>=8.0.1.1.0",
# ]
# ///
"""Independent OCCT measurements for the STEP probe; diagnostic use only.

This external oracle is never a dependency of the Rust application.
"""

import argparse
import json
from pathlib import Path

from OCP.Bnd import Bnd_Box
from OCP.BRepBndLib import BRepBndLib
from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.BRepGProp import BRepGProp
from OCP.GProp import GProp_GProps
from OCP.IFSelect import IFSelect_RetDone
from OCP.STEPControl import STEPControl_Reader
from OCP.TopAbs import TopAbs_FACE, TopAbs_SOLID
from OCP.TopExp import TopExp_Explorer


def read_shape(path):
    reader = STEPControl_Reader()
    if reader.ReadFile(str(path)) != IFSelect_RetDone:
        raise ValueError(f"cannot read {path}")
    reader.TransferRoots()
    shape = reader.OneShape()
    if shape.IsNull():
        raise ValueError(f"no geometry: {path}")
    return shape


def measure_shape(shape):
    bounds = Bnd_Box()
    BRepBndLib.AddOptimal_s(shape, bounds, False, False)
    volume, area = GProp_GProps(), GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, volume)
    BRepGProp.SurfaceProperties_s(shape, area)

    def count(kind):
        iterator = TopExp_Explorer(shape, kind)
        count = 0
        while iterator.More():
            count += 1
            iterator.Next()
        return count

    low, high = bounds.CornerMin(), bounds.CornerMax()
    center = volume.CentreOfMass()
    return {
        "bounds": [[low.X(), low.Y(), low.Z()], [high.X(), high.Y(), high.Z()]],
        "volume": volume.Mass(),
        "center": [center.X(), center.Y(), center.Z()],
        "area": area.Mass(),
        "faces": count(TopAbs_FACE),
        "solids": count(TopAbs_SOLID),
        "valid": BRepCheck_Analyzer(shape).IsValid(),
    }


def measure(path):
    return measure_shape(read_shape(path))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("summary", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    results = []
    for case in json.loads(args.summary.read_text()):
        reference = measure(Path(case["input"]))
        expected_file = Path(case["input"]).with_suffix(".expected.json")
        if expected_file.exists():
            expected = json.loads(expected_file.read_text())
            assert reference["valid"] and reference["solids"] == expected["solids"]
            assert abs(reference["volume"] - expected["volume"]) < max(
                1e-7, abs(expected["volume"]) * 1e-9
            )
            # OCCT enlarges analytic bounds by its 1e-7 modelling tolerance.
            assert all(
                abs(reference["bounds"][side][axis] - expected["bounds"][side][axis])
                < 2e-7
                for side in range(2)
                for axis in range(3)
            )
        roundtrips = []
        for shell in case.get("report", {}).get("shells", []):
            if "roundtrip_file" in shell:
                roundtrips.append(measure(Path(shell["roundtrip_file"])))
        data = case["report"]
        assembly = data["assembly"]
        exported = (
            measure(Path(assembly["roundtrip_file"]))
            if "roundtrip_file" in assembly
            else None
        )
        meshes = [
            shell
            for instance in assembly.get("instances", [])
            for shell in instance["shells"]
        ]
        mesh_volume = sum(mesh["signed_volume"] for mesh in meshes)
        mesh_bounds = (
            [
                [
                    operation(mesh["bounds"][side][axis] for mesh in meshes)
                    for axis in range(3)
                ]
                for side, operation in enumerate([min, max])
            ]
            if meshes
            else None
        )
        bound_error = (
            max(
                abs(mesh_bounds[side][axis] - reference["bounds"][side][axis])
                for side in range(2)
                for axis in range(3)
            )
            if meshes
            else None
        )
        roundtrip_ok = (
            exported is not None
            and exported["valid"]
            and exported["solids"] == reference["solids"]
            and exported["faces"] == reference["faces"]
            and abs(exported["volume"] - reference["volume"])
            <= max(1e-7, abs(reference["volume"]) * 1e-7)
            and all(
                abs(exported["bounds"][side][axis] - reference["bounds"][side][axis])
                <= 1e-6
                for side in range(2)
                for axis in range(3)
            )
        )
        mesh_ok = (
            assembly.get("solid_count") == reference["solids"]
            and bound_error is not None
            and bound_error <= data["tolerance"] * 2
            and abs(mesh_volume - reference["volume"])
            <= reference["area"] * data["tolerance"] * 2
        )
        result = {
            "input": case["input"],
            "reference": reference,
            "roundtrips": roundtrips,
            "assembly_roundtrip": exported,
            "roundtrip_pass": roundtrip_ok,
            "mesh_pass": mesh_ok,
            "mesh_volume": mesh_volume,
            "mesh_bounds_error": bound_error,
        }
        results.append(result)
        print(
            Path(case["input"]).name,
            "solids",
            reference["solids"],
            "roundtrip",
            roundtrip_ok,
            "mesh",
            mesh_ok,
            flush=True,
        )
    args.output.write_text(json.dumps(results, indent=2) + "\n")
    if not all(result["roundtrip_pass"] and result["mesh_pass"] for result in results):
        raise SystemExit(1)
