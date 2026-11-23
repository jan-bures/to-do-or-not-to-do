#!/usr/bin/env python3
"""Reject local or denied bytes in the Windows update feed, packages and installer."""

import argparse
from pathlib import Path
import sys
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "scripts"))
from release_privacy import ScanFailure, needles, scan


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("release_dir", type=Path)
    parser.add_argument("--require-denylist", action="store_true")
    parser.add_argument("--notes-stdin", action="store_true", help="scan release notes read from stdin")
    args = parser.parse_args()
    try:
        patterns = tuple(needles(android=False, require_denylist=args.require_denylist))
        root = args.release_dir
        packages = sorted(root.glob("*.nupkg"))
        if not any(path.name.endswith("-full.nupkg") for path in packages):
            raise ScanFailure("No full package to scan")
        notes = sys.stdin.buffer.read(1024 * 1024 + 1) if args.notes_stdin else b""
        if len(notes) > 1024 * 1024:
            raise ScanFailure("Release notes exceed the scan limit")
        scan(root / "releases.win.json", patterns, cert_info=notes,
             label="Windows feed", metadata_label="Release notes", archive_required=False)
        proof = root / "desktop-update.sig.json"
        if proof.is_file():
            scan(proof, patterns, label="Windows feed signature", archive_required=False)
        for package in packages:
            scan(package, patterns, label="Windows package", forbid_pdb=True)
        scan(root / "todo-desktop-win-Setup.exe", patterns, label="Windows installer", forbid_pdb=True)
    except (OSError, RuntimeError, ValueError, zipfile.BadZipFile, NotImplementedError) as error:
        # Do not disclose local paths or entry names from library exceptions.
        detail = str(error) if isinstance(error, ScanFailure) else type(error).__name__
        parser.exit(1, f"Release privacy scan failed: {detail}\n")
    print("Windows release privacy scan passed.")


if __name__ == "__main__":
    main()
