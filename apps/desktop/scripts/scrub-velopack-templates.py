#!/usr/bin/env python3
"""Remap build-source paths embedded in vpk's pinned, unsigned Windows templates."""

import argparse
from hashlib import sha256
from pathlib import Path
import tempfile

# These three vpk 1.2.161 x64 templates contain Rust file locations from upstream's
# public CI runner. vpk copies them into the installer/package verbatim apart from
# resources and the bundle pointer. Remap the fixed-width prefix BEFORE vpk hashes
# and packages them. Review the upstream binaries whenever vpk is bumped.
TEMPLATES = {
    "setup_x64.exe": "b4bac7fed188b173d36bac27f59376dfcac59c8db3f830858772b244c2308cab",
    "stub_x64.exe": "405517cdd4fb8d106ada4ce1fd834b583b46c3a3e2a9da12b2d3c4b6d69999b5",
    "update_x64.exe": "acefb4a2cb46cc77ed3c2364db17f8bd2a25e2197cfeae56cd85e88a7ac21ac5",
}
UPSTREAM = rb"C:\Users\runneradmin"
REMAPPED = b"/velopack-build-home"
assert len(UPSTREAM) == len(REMAPPED)


def remap(data, expected):
    if sha256(data).hexdigest() == expected:
        if UPSTREAM not in data or REMAPPED in data:
            raise ValueError("Unexpected Velopack template layout")
        return data.replace(UPSTREAM, REMAPPED)
    if REMAPPED in data and UPSTREAM not in data and sha256(data.replace(REMAPPED, UPSTREAM)).hexdigest() == expected:
        return data  # A subsequent package run can reuse the same worktree-local tool.
    raise ValueError("Unrecognized Velopack template; review the pinned vpk version")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("templates", type=Path)
    args = parser.parse_args()
    try:
        for name, expected in TEMPLATES.items():
            path = args.templates / name
            original = path.read_bytes()
            clean = remap(original, expected)
            if clean is not original:
                with tempfile.TemporaryDirectory(dir=path.parent) as staging:
                    staged = Path(staging) / name
                    staged.write_bytes(clean)
                    staged.replace(path)
    except (ValueError, OSError):
        parser.exit(1, "Cannot scrub the pinned Velopack templates. Review or reinstall vpk.\n")


if __name__ == "__main__":
    main()
