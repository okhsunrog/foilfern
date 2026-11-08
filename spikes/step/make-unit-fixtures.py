"""Derive unit probes from the generated analytic plate, outside the repository."""

import argparse
import json
import re
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("plate_binary", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    unit = "#12 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );"
    for name, factor, prefix in [
        ("centimetres", 10.0, ".CENTI."),
        ("metres", 1000.0, "$"),
        ("inches", 25.4, None),
    ]:
        path = args.output / f"{name}.step"
        # Keep the physical plate at 20 x 10 mm in every unit system.
        subprocess.run(
            [str(args.plate_binary.resolve()), str(path), str(1.0 / factor)], check=True
        )
        source = path.read_text()
        expected = json.loads(path.with_suffix(".expected.json").read_text())
        assert source.count(unit) == 1
        last = max(int(value) for value in re.findall(r"#(\d+)\s*=", source)) + 1
        replacement = (
            unit.replace(".MILLI.", prefix)
            if prefix
            else f"#12 = ( CONVERSION_BASED_UNIT('inch',#{last}) LENGTH_UNIT() NAMED_UNIT(*) );"
        )
        text = source.replace(unit, replacement)
        if name == "inches":
            text = text.replace(
                "ENDSEC;\nEND-ISO",
                f"#{last} = LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#{last + 1});\n#{last + 1} = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );\nENDSEC;\nEND-ISO",
            )
        path.write_text(text)
        scaled = {
            "volume": expected["volume"] * factor**3,
            "bounds": [[v * factor for v in bound] for bound in expected["bounds"]],
            "solids": expected["solids"],
        }
        path.with_suffix(".expected.json").write_text(
            json.dumps(scaled, indent=2) + "\n"
        )


if __name__ == "__main__":
    main()
