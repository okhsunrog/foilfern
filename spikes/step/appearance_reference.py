# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "cadquery-ocp-novtk>=8.0.1.1.0",
# ]
# ///
"""Independent XCAF face-appearance comparison for original and exported STEP."""

import argparse
import json
import re
from collections import defaultdict
from pathlib import Path

from OCP.BRepGProp import BRepGProp
from OCP.collections import (
    IndexedDataMap_TopoDS_Shape_XCAFPrs_Style_TopTools_ShapeMapHasher,
    IndexedMap_TopoDS_Shape_TopTools_ShapeMapHasher,
    Sequence_TDF_Label,
)
from OCP.GProp import GProp_GProps
from OCP.IFSelect import IFSelect_RetDone
from OCP.Quantity import Quantity_TOC_sRGB
from OCP.STEPCAFControl import STEPCAFControl_Reader
from OCP.TCollection import TCollection_ExtendedString
from OCP.TDocStd import TDocStd_Document
from OCP.TopAbs import TopAbs_FACE, TopAbs_FORWARD, TopAbs_SOLID
from OCP.TopExp import TopExp_Explorer
from OCP.TopLoc import TopLoc_Location
from OCP.XCAFDoc import XCAFDoc_DocumentTool
from OCP.XCAFPrs import XCAFPrs
from reference import measure_shape


def faces(shape):
    iterator = TopExp_Explorer(shape, TopAbs_FACE)
    while iterator.More():
        yield iterator.Current().Oriented(TopAbs_FORWARD)
        iterator.Next()


def appearances(path, exclude_solid_bounds=()):
    reader = STEPCAFControl_Reader()
    reader.SetColorMode(True)
    if reader.ReadFile(str(path)) != IFSelect_RetDone:
        raise ValueError(f"cannot read {path}")
    document = TDocStd_Document(TCollection_ExtendedString("MDTV-XCAF"))
    assert reader.Transfer(document)
    shape_tool = XCAFDoc_DocumentTool.ShapeTool_s(document.Main())
    roots = Sequence_TDF_Label()
    shape_tool.GetFreeShapes(roots)
    styles = IndexedDataMap_TopoDS_Shape_XCAFPrs_Style_TopTools_ShapeMapHasher()
    for index in range(1, roots.Length() + 1):
        XCAFPrs.CollectStyleSettings_s(roots.Value(index), TopLoc_Location(), styles)
    colors = {}
    colored_shapes = IndexedMap_TopoDS_Shape_TopTools_ShapeMapHasher()
    # Broad inherited styles first; explicit face overrides last.
    for face_only in [False, True]:
        for index in range(1, styles.Extent() + 1):
            shape, style = styles.FindKey(index), styles.FindFromIndex(index)
            if (
                shape.ShapeType() == TopAbs_FACE
            ) != face_only or not style.IsSetColorSurf():
                continue
            rgba = style.GetColorSurfRGBA()
            rgb = rgba.GetRGB()
            color = [*rgb.Values(Quantity_TOC_sRGB), rgba.Alpha()]
            targets = [shape.Oriented(TopAbs_FORWARD)] if face_only else faces(shape)
            for face in targets:
                colors[colored_shapes.Add(face)] = color
    result = []
    for index in range(1, roots.Length() + 1):
        shape = shape_tool.GetShape_s(roots.Value(index))
        solids = TopExp_Explorer(shape, TopAbs_SOLID)
        parts = []
        while solids.More():
            part = solids.Current()
            bounds = measure_shape(part)["bounds"]
            if not any(
                max(
                    abs(bounds[side][axis] - excluded[side][axis])
                    for side in range(2)
                    for axis in range(3)
                )
                < 1e-6
                for excluded in exclude_solid_bounds
            ):
                parts.append(part)
            solids.Next()
        # The probe's supported inputs are solid models; do not silently pass
        # an empty appearance comparison for surface-only input.
        for face in [face for part in parts for face in faces(part)]:
            metric = measure_shape(face)
            surface = GProp_GProps()
            BRepGProp.SurfaceProperties_s(face, surface)
            center = surface.CentreOfMass()
            result.append(
                {
                    "bounds": metric["bounds"],
                    "area": surface.Mass(),
                    "center": [center.X(), center.Y(), center.Z()],
                    "color": colors.get(colored_shapes.FindIndex(face)),
                }
            )
    assert not colors or any(face["color"] is not None for face in result), (
        "XCAF colors did not resolve to actual faces"
    )
    return result


def key(face):
    return tuple(
        round(value, 4) for bound in face["bounds"] for value in bound
    ) + tuple(round(value, 4) for value in face["center"])


def matches(face, candidate):
    if (
        max(
            abs(face["bounds"][side][axis] - candidate["bounds"][side][axis])
            for side in range(2)
            for axis in range(3)
        )
        > 1e-6
    ):
        return False
    if (
        max(
            abs(a - b) for a, b in zip(face["center"], candidate["center"], strict=True)
        )
        > 1e-6
    ):
        return False
    if (face["color"] is None) != (candidate["color"] is None):
        return False
    if (
        face["color"] is not None
        and max(
            abs(a - b) for a, b in zip(face["color"], candidate["color"], strict=True)
        )
        > 2e-7
    ):
        return False
    return abs(face["area"] - candidate["area"]) <= max(
        1e-7, abs(candidate["area"]) * 1e-7
    )


def compare(original, exported, exclude_solid_bounds=()):
    expected = appearances(original, exclude_solid_bounds)
    known = original.with_suffix(".appearance.json")
    if known.exists():
        for material in json.loads(known.read_text())["materials"]:
            rgba = [*material["color"], material["opacity"]]
            count = sum(
                face["color"] is not None
                and max(abs(a - b) for a, b in zip(face["color"], rgba, strict=True))
                < 2e-7
                for face in expected
            )
            assert count == material["faces"], (material, count)
    actual = appearances(exported, exclude_solid_bounds)
    assert len(expected) == len(actual), (original, len(expected), len(actual))
    groups = defaultdict(list)
    for face in expected:
        groups[key(face)].append(face)
    for face in actual:
        candidates = groups[key(face)]
        match = next(
            (
                index
                for index, candidate in enumerate(candidates)
                if matches(face, candidate)
            ),
            None,
        )
        if match is None:
            # Values exactly on a rounding boundary may land in neighboring
            # buckets after a harmless 1e-14 placement difference. Check actual
            # geometric tolerances rather than weakening the comparison.
            for bucket in groups.values():
                match = next(
                    (
                        index
                        for index, candidate in enumerate(bucket)
                        if matches(face, candidate)
                    ),
                    None,
                )
                if match is not None:
                    candidates = bucket
                    break
        assert match is not None, (
            f"unmatched face appearance in {exported}: {face} candidates: {candidates}"
        )
        candidates.pop(match)
    assert not any(groups.values())
    return {
        "input": str(original),
        "faces": len(expected),
        "colored_faces": sum(face["color"] is not None for face in expected),
        "passed": True,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("summary", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument(
        "--negative-control",
        type=Path,
        help="directory for a deliberately recolored export",
    )
    args = parser.parse_args()
    results = []
    for case in json.loads(args.summary.read_text()):
        result = compare(
            Path(case["input"]), Path(case["report"]["assembly"]["roundtrip_file"])
        )
        results.append(result)
        print(
            Path(case["input"]).name,
            result["colored_faces"],
            "colored faces: pass",
            flush=True,
        )
    if args.negative_control:
        case = next(
            case
            for case, result in zip(
                json.loads(args.summary.read_text()), results, strict=True
            )
            if result["colored_faces"]
        )
        args.negative_control.mkdir(parents=True, exist_ok=True)
        path = args.negative_control / "recolored.step"
        source = Path(case["report"]["assembly"]["roundtrip_file"])
        changed, count = re.subn(
            r"COLOUR_RGB\('[^']*',[^)]+\)",
            "COLOUR_RGB('',0.5,0.5,0.5)",
            source.read_text(),
        )
        assert count
        path.write_text(changed)
        try:
            compare(Path(case["input"]), path)
        except AssertionError:
            print("negative control: recoloring rejected", flush=True)
        else:
            raise AssertionError("appearance oracle missed recoloring")
    args.output.write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
