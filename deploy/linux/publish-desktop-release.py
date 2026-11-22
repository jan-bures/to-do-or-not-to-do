#!/usr/bin/env python3
"""Publish a trusted local vpk output directory to todo-server's private desktop feed."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import tempfile


INDEX = "releases.win.json"
SETUP = "todo-desktop-win-Setup.exe"
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-build\.([1-9][0-9]*)")
MAX_PACKAGE = 256 * 1024 * 1024
MAX_FEED = 1024 * 1024


def regular(path):
    """Refuse links, directories and special files."""
    if not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError(f"not a regular file: {path.name}")
    return path


def fingerprint(path):
    with regular(path).open("rb") as file:
        digest = hashlib.sha256()
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
        return digest.hexdigest(), os.fstat(file.fileno()).st_size


def feed(path):
    raw = regular(path).read_bytes()
    if len(raw) > MAX_FEED:
        raise ValueError("release feed is too large")
    assets = json.loads(raw)["Assets"]
    if not isinstance(assets, list) or not assets:
        raise ValueError("release feed has no assets")
    packages = {}
    latest = None
    for asset in assets:
        version = VERSION.fullmatch(asset["Version"])
        kind = asset["Type"]
        if not version or kind not in ("Full", "Delta") or asset["PackageId"] != "todo-desktop":
            raise ValueError("unexpected desktop release metadata")
        number = tuple(map(int, version.groups()))
        if number[-1] > 2**32 - 1:
            raise ValueError("desktop build number is out of range")
        name = f"todo-desktop-{version[0]}-{kind.lower()}.nupkg"
        digest, size = asset["SHA256"], asset["Size"]
        if name != asset["FileName"] or name in packages or not re.fullmatch(r"[a-fA-F0-9]{64}", digest):
            raise ValueError("invalid or duplicate desktop package name or hash")
        if type(size) is not int or not 0 < size <= MAX_PACKAGE:
            raise ValueError("desktop package size is out of range")
        packages[name] = (digest.lower(), size)
        if kind == "Full":
            latest = number if latest is None else max(latest, number)
    if latest is None:
        raise ValueError("release feed has no full package")
    return latest, packages, raw


def publish(source, dest, setup_sha256=None):
    if setup_sha256 is not None and not re.fullmatch(r"[a-fA-F0-9]{64}", setup_sha256):
        raise ValueError("expected a 64-digit hexadecimal Setup.exe SHA-256")
    dest_meta = dest.lstat()
    if not stat.S_ISDIR(dest_meta.st_mode) or dest_meta.st_uid != os.geteuid() or dest_meta.st_mode & 0o022:
        raise ValueError("destination must be a plain directory only the publishing user can write")
    # Prevent two publishes from racing. Temp files are on the destination filesystem, so every
    # rename is atomic. Keep old packages: a download may be using the previous feed.
    with (dest / ".publish.lock").open("ab") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        version, packages, manifest = feed(source / INDEX)
        try:
            previous, _, _ = feed(dest / INDEX)
        except FileNotFoundError:
            previous = None
        if previous is not None and version <= previous:
            raise ValueError("new desktop release must be newer than the published feed")

        with tempfile.TemporaryDirectory(prefix=".staging-", dir=dest) as scratch:
            staging = Path(scratch)
            for name, expected in packages.items():
                installed = dest / name
                try:
                    actual = fingerprint(installed)
                except FileNotFoundError:
                    # Verify the bytes actually staged, not a separate read of the upload.
                    shutil.copyfile(regular(source / name), staging / name)
                    if fingerprint(staging / name) != expected:
                        raise ValueError(f"package hash or size does not match the feed: {name}")
                else:
                    if actual != expected:
                        raise ValueError(f"refusing to replace published package bytes: {name}")

            setup = staging / SETUP
            shutil.copyfile(regular(source / SETUP), setup)
            with setup.open("rb") as file:
                if file.read(2) != b"MZ":
                    raise ValueError("Setup.exe is not a Windows executable")
            if setup_sha256 is not None and fingerprint(setup)[0] != setup_sha256.lower():
                raise ValueError("staged Setup.exe SHA-256 does not match")
            for file in staging.iterdir():
                install(file, dest)
            sync_directory(dest)
            (staging / INDEX).write_bytes(manifest)
            install(staging / INDEX, dest)
            sync_directory(dest)
    print(f"Published desktop release {version[0]}.{version[1]}.{version[2]}-build.{version[3]}")


def install(path, dest):
    path.chmod(0o644)
    with path.open("rb") as file:
        os.fsync(file.fileno())
    path.replace(dest / path.name)


def sync_directory(path):
    fd = os.open(path, os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="uploaded vpk output directory")
    parser.add_argument("dest", type=Path, help="existing [releases] dir")
    parser.add_argument("--setup-sha256", help="expected SHA-256 of Setup.exe from the trusted build output")
    args = parser.parse_args()
    try:
        publish(args.source, args.dest, setup_sha256=args.setup_sha256)
    except (OSError, ValueError, TypeError, KeyError) as error:
        parser.exit(1, f"Cannot publish desktop release: {error}\n")


if __name__ == "__main__":
    main()
