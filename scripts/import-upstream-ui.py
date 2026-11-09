#!/usr/bin/env python3
"""Imports upstream LibrePCB's Slint UI into crates/librepcb-app/ui.

Copies all `.slint` files of `libs/librepcb/ui` and the resources they
reference (images, icons and fonts outside of `libs/librepcb/ui`), and
rewrites the resource paths from the upstream repository layout to
`ui/resources/<path relative to the upstream repository root>`.

Usage:
    scripts/import-upstream-ui.py [--font-awesome DIR] [--bootstrap-icons DIR]
                                  [--fonts DIR]

The upstream checkout is taken from $LIBREPCB_UPSTREAM_DIR (default
../LibrePCB). The optional directories replace the upstream submodules
`libs/font-awesome`, `libs/bootstrap-icons` and `share/librepcb/fonts` when
they are not checked out (clone them at the commits upstream pins).

The script overwrites the `.slint` files, so local changes to them must be
re-applied afterwards (see crates/librepcb-app/ui/PROVENANCE.md). Files of
this repository (`Cargo.toml`, `build.rs`, `src/`, `PROVENANCE.md`) are not
touched.
"""

import argparse
import os
import re
import shutil
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DST = REPO / "crates" / "librepcb-app" / "ui"
RESOURCES = "resources"

# Resources used by the Rust backend (crates/librepcb-app/src/icons.rs), not
# referenced by any `.slint` file.
BACKEND_RESOURCES = [
    "img/app/librepcb.png",
    "img/places/file.png",
    "img/places/folder.png",
    "img/places/project_folder.png",
]

# Matches `@image-url("...")` and `import "...";` (font imports).
PATTERN = re.compile(r'(@image-url\(\s*"|import\s+")([^"]+)(")')


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--font-awesome", type=Path)
    parser.add_argument("--bootstrap-icons", type=Path)
    parser.add_argument("--fonts", type=Path)
    args = parser.parse_args()

    upstream = Path(
        os.environ.get("LIBREPCB_UPSTREAM_DIR", REPO.parent / "LibrePCB")
    ).resolve()
    src = upstream / "libs" / "librepcb" / "ui"
    overrides = {
        "libs/font-awesome": args.font_awesome,
        "libs/bootstrap-icons": args.bootstrap_icons,
        "share/librepcb/fonts": args.fonts,
    }

    def locate(rel: str) -> Path:
        for prefix, directory in overrides.items():
            if directory and rel.startswith(prefix + "/"):
                return directory / rel[len(prefix) + 1 :]
        return upstream / rel

    resources: set[str] = set(BACKEND_RESOURCES)
    for slint in sorted(src.rglob("*.slint")):
        rel_file = slint.relative_to(src)
        text = slint.read_text(encoding="utf-8")
        depth = len(rel_file.parts) - 1

        def rewrite(m: re.Match) -> str:
            path = m.group(2)
            if path.endswith(".slint") or not path.startswith(".."):
                return m.group(0)
            resolved = os.path.normpath(slint.parent / path)
            root_rel = os.path.relpath(resolved, upstream)
            if root_rel.startswith("libs/librepcb/ui/"):
                return m.group(0)
            resources.add(root_rel)
            new = "../" * depth + RESOURCES + "/" + root_rel
            return m.group(1) + new + m.group(3)

        out = DST / rel_file
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(PATTERN.sub(rewrite, text), encoding="utf-8")

    missing = []
    for rel in sorted(resources):
        source = locate(rel)
        if not source.is_file():
            missing.append(rel)
            continue
        target = DST / RESOURCES / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    print(f"{len(resources) - len(missing)} resources copied")
    for rel in missing:
        print(f"missing: {rel}", file=sys.stderr)
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
