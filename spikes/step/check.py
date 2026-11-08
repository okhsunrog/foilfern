"""Run isolated, time-bounded geometry probes over external STEP fixtures."""

import argparse
import hashlib
import json
import os
import subprocess
import zipfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path, PurePosixPath


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument(
        "--upstream", type=Path, default=os.environ.get("LIBREPCB_UPSTREAM_DIR")
    )
    parser.add_argument("--input", type=Path, action="append", default=[])
    parser.add_argument("--tolerance", type=float, default=0.01)
    parser.add_argument("--timeout", type=float, default=45.0)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    inputs = args.input
    if args.archive:
        with zipfile.ZipFile(args.archive) as archive:
            for name in sorted(archive.namelist()):
                if not name.lower().endswith((".step", ".stp")):
                    continue
                path = PurePosixPath(name)
                if path.is_absolute() or ".." in path.parts:
                    raise ValueError(f"unsafe archive path: {name}")
                target = args.output / "inputs" / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(archive.read(name))
                inputs.append(target)
    if args.upstream:
        root = args.upstream / "tests/data/unittests/librepcbcommon/OccModelTest"
        inputs.extend(sorted(root.glob("*.step")))
    if not inputs:
        parser.error("no STEP inputs")
    unique = {}
    for path in inputs:
        unique.setdefault(hashlib.sha256(path.read_bytes()).hexdigest(), path.resolve())

    def run(case):
        index, path = case
        output = args.output / "runs" / f"{index:02}-{path.stem}"
        output.mkdir(parents=True, exist_ok=True)
        report = {"input": str(path), "output": str(output)}
        (output / "report.json").unlink(missing_ok=True)
        try:
            result = subprocess.run(
                [
                    str(args.binary.resolve()),
                    str(path),
                    str(output),
                    str(args.tolerance),
                ],
                capture_output=True,
                text=True,
                errors="replace",
                timeout=args.timeout,
                check=False,
            )
            (output / "stdout.log").write_text(result.stdout)
            (output / "stderr.log").write_text(result.stderr)
            report["exit"] = result.returncode
            if (output / "report.json").exists():
                data = json.loads((output / "report.json").read_text())
                report["report"] = data
                report["geometry_pass"] = (
                    result.returncode == 0
                    and data["entity_losses"] == 0
                    and bool(data["shells"])
                    and all(
                        "error" not in shell
                        and shell["losses"] == 0
                        and shell["missing_mesh_faces"] == 0
                        and shell["roundtrip_losses"] == 0
                        and shell["roundtrip_missing_mesh_faces"] == 0
                        and all(
                            shell[mesh][key] == 0
                            for mesh in ["mesh", "roundtrip"]
                            for key in [
                                "boundary_edges",
                                "nonmanifold_edges",
                                "misoriented_edges",
                            ]
                        )
                        for shell in data["shells"]
                    )
                )
                assembly = data["assembly"]
                report["geometry_pass"] = (
                    report["geometry_pass"]
                    and "error" not in assembly
                    and all(
                        all(
                            mesh[key] == 0
                            for key in [
                                "boundary_edges",
                                "nonmanifold_edges",
                                "misoriented_edges",
                            ]
                        )
                        for instance in assembly.get("instances", [])
                        for mesh in instance["shells"]
                    )
                )
            else:
                report["geometry_pass"] = False
        except subprocess.TimeoutExpired:
            report["timeout"] = args.timeout
            report["geometry_pass"] = False
        print(index, path.name, "geometry pass:", report["geometry_pass"], flush=True)
        return report

    with ThreadPoolExecutor(max_workers=2) as pool:
        reports = list(pool.map(run, enumerate(unique.values())))
    (args.output / "summary.json").write_text(json.dumps(reports, indent=2) + "\n")
    if not all(case["geometry_pass"] for case in reports):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
