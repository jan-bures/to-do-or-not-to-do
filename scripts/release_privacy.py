#!/usr/bin/env python3
"""Scan raw and expanded release bytes for local paths and private values."""

import argparse
import getpass
import os
from pathlib import Path, PurePosixPath
import re
import socket
import subprocess
import sys
import tempfile
import zipfile


REPO_ROOT = Path(__file__).resolve().parents[1]
ARCHIVES = {".zip", ".jar", ".apk", ".nupkg"}
ENTRY_TYPES = ARCHIVES | {".dex", ".so", ".xml", ".arsc", ".exe", ".dll", ".pdb"}
CHUNK_SIZE = 1024 * 1024
MAX_NESTING = 8
MAX_EXPANDED = 512 * 1024 * 1024
# Toolchains and native libraries may contain public fallback directory names. Scan children
# of these shared roots, not the bare names or substrings of unrelated paths.
GENERIC_SYSTEM_DIRS = {"/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp", "/usr", "/var", "/run", "/opt"}
IDENTITY_TOKENS = {"local user name", "git author name", "machine hostname"}
MIN_IDENTITY_LENGTH = 3


class ScanFailure(ValueError):
    """Safe to log: its messages contain only fixed labels and categories."""


def spellings(value):
    windows = value.replace("/", "\\")
    return {value, value.replace("\\", "/"), windows, windows.replace("\\", "\\\\")}


def command_value(root, args, optional=False):
    result = subprocess.run(args, cwd=root, capture_output=True, encoding="utf-8", errors="replace", check=False)
    if optional and result.returncode == 1:
        return ""  # git config --get: not configured
    if result.returncode:
        raise RuntimeError("could not determine a local scan pattern")
    return result.stdout.strip()


def path_needles(root, android):
    home = Path.home()
    paths = {
        "workspace root": [root],
        "CARGO_HOME": [home / ".cargo"],
        "RUSTUP_HOME": [home / ".rustup"],
        "temp dir": [Path(tempfile.gettempdir())],
    }
    if sysroot := command_value(root, ["rustc", "--print", "sysroot"]):
        paths["Rust sysroot"] = [Path(sysroot)]
    variables = ("CARGO_HOME", "RUSTUP_HOME", "TMP", "TEMP", "TMPDIR")
    if android:
        paths["GRADLE_USER_HOME"] = [home / ".gradle"]
        variables += ("GRADLE_USER_HOME", "ANDROID_HOME", "ANDROID_SDK_ROOT", "ANDROID_NDK_HOME", "ANDROID_NDK_ROOT")
    for key in variables:
        if value := os.getenv(key):
            category = "temp dir" if key in {"TMP", "TEMP", "TMPDIR"} else key
            paths.setdefault(category, []).append(Path(value))
    # Gradle can get its SDK from local.properties even when the environment is empty.
    properties = root / "apps" / "android" / "local.properties"
    if android and properties.is_file():
        for line in properties.read_text(encoding="utf-8-sig").splitlines():
            if found := re.fullmatch(r"\s*(?:sdk|ndk)\.dir\s*[=:]\s*(.+?)\s*", line):
                paths.setdefault("Android SDK/NDK (local.properties)", []).append(Path(re.sub(r"\\([\\ :])", r"\1", found[1])))
    values = set()
    for category, directories in paths.items():
        for path in directories:
            path = path if path.is_absolute() else root / path
            for absolute in (path.absolute(), path.resolve()):
                if absolute != Path(absolute.anchor):
                    generic = absolute.as_posix().lower() in GENERIC_SYSTEM_DIRS
                    values.update((spelling.lower(), category, generic) for spelling in spellings(str(absolute)))
    # Report the most specific path first, e.g. the sysroot rather than its RUSTUP_HOME parent.
    return sorted(values, key=lambda value: (-len(value[0]), value[1], value[0]))


def denylist_values(path):
    lines = Path(path).read_text(encoding="utf-8-sig").splitlines()
    values = {line.strip() for line in lines if line.strip() and not line.lstrip().startswith("#")}
    if not values:
        raise ScanFailure("Private denylist contains no active patterns")
    return values


def needles(root=REPO_ROOT, *, android=True, require_denylist=False):
    paths = {str(Path.home()), os.getenv("USERPROFILE", ""), os.getenv("HOME", "")}
    values = {
        "home path": {form for path in paths if path for form in spellings(path)},
        "local user name": {getpass.getuser(), os.getenv("USERNAME", ""), os.getenv("USER", "")},
        "user directory prefix": spellings(r"C:\Users") | spellings("/home/") | spellings("/Users/"),
        "git author name": {command_value(root, ["git", "config", "--get", "user.name"], optional=True)},
        "git author email": {command_value(root, ["git", "config", "--get", "user.email"], optional=True)},
        "machine hostname": {socket.gethostname(), os.getenv("COMPUTERNAME", ""), os.getenv("HOSTNAME", "")},
    }
    key = "TODO_ANDROID_DENYLIST" if android else "TODO_DESKTOP_DENYLIST"
    if denylist := os.getenv(key):
        values["private denylist"] = denylist_values(denylist)
    elif require_denylist:
        raise ScanFailure("A private denylist is required")
    encodings = ("utf-8", "utf-16le", "utf-16be")
    for text, category, generic in path_needles(root, android):
        for encoding in encodings:
            yield pattern(text, encoding, category, path_prefix=generic)
    for reason, strings in values.items():
        for text in sorted({value.lower() for value in strings if value}):
            # One- and two-character machine/account names are too ambiguous as standalone
            # binary tokens. Concrete paths and explicit denylist entries have no length floor.
            if reason in IDENTITY_TOKENS and len(text) < MIN_IDENTITY_LENGTH:
                continue
            for encoding in encodings:
                yield pattern(text, encoding, reason)


def pattern(text, encoding, reason, path_prefix=False):
    raw = text.encode(encoding).lower()
    width = len(raw)
    expression = re.escape(raw)
    if not text.isascii():
        # bytes.lower() only folds ASCII. Match encoded case variants without assuming where
        # text starts in binary resources, including unaligned UTF-16 in the signing block.
        choices = [{item.encode(encoding).lower() for item in (c, c.upper(), c.title(), c.casefold())} for c in text]
        expression = b"".join(b"(?:" + b"|".join(map(re.escape, sorted(parts))) + b")" if len(parts) > 1
                              else re.escape(next(iter(parts))) for parts in choices)
        width = sum(max(map(len, parts)) for parts in choices)
    # An identity is a token, not the suffix of e.g. ApiCallRunner.
    bounded = reason in IDENTITY_TOKENS

    def code_unit(ascii_class):
        if encoding == "utf-16le":
            return ascii_class + b"\x00"
        if encoding == "utf-16be":
            return b"\x00" + ascii_class
        return ascii_class

    if path_prefix:
        # Left boundary excludes relative paths and longer prefixes (e.g. /var/tmp for /tmp).
        preceding = code_unit(rb"[a-z0-9_./\\-]")
        separator = code_unit(rb"[/\\]")
        expression = b"(?<!" + preceding + b")" + expression + b"(?=" + separator + b")"
    elif bounded:
        word = code_unit(b"[a-z0-9_]")
        expression = b"(?<!" + word + b")" + expression + b"(?!" + word + b")"
    return (raw if text.isascii() else None), reason, (re.compile(expression) if path_prefix or bounded or not text.isascii() else None), width


def scan(path, patterns, cert_info=b"", *, label="APK", archive_required=True, forbid_pdb=False):
    """Cover raw bytes, expanded entries, and nested ZIPs without extracting paths to disk."""
    def match(data, has_prefix=False, has_suffix=False):
        data = data.lower()
        for word, reason, expression, _ in patterns:
            if word is not None and word not in data:
                continue
            if expression is None or any((not has_prefix or hit.start() >= 2) and (not has_suffix or hit.end() <= len(data) - 2)
                                         for hit in expression.finditer(data)):
                return reason
        return None

    overlap = max(width for _, _, _, width in patterns) + 4
    def match_stream(body, size):
        previous = b""
        seen = 0
        while chunk := body.read(CHUNK_SIZE):
            data = previous + chunk
            reason = match(data, has_prefix=seen > len(previous), has_suffix=seen + len(chunk) < size)
            if reason:
                return reason
            seen += len(chunk)
            previous = data[-overlap:]
        return None

    expanded = 0

    def inspect_archive(source, label, depth):
        nonlocal expanded
        if depth > MAX_NESTING:
            raise ScanFailure("Too many nested archives")
        with zipfile.ZipFile(source) as archive:
            if found := match(archive.comment):
                raise ScanFailure(f"{label} metadata matched {found}")
            for index, entry in enumerate(archive.infolist(), start=1):
                # Don't print entry names: one could itself include a private value.
                kind = PurePosixPath(entry.filename).suffix.lower()
                kind = kind if kind in ENTRY_TYPES else "other"
                entry_label = f"{label} entry {index} ({kind})"
                if forbid_pdb and kind == ".pdb":
                    raise ScanFailure(f"{entry_label} contains a debug symbols file")
                expanded += entry.file_size
                if expanded > MAX_EXPANDED:
                    raise ScanFailure("Expanded entries exceed the scan limit")
                reason = match(entry.filename.encode("utf-8")) or match(entry.comment) or match(entry.extra)
                with archive.open(entry) as body:
                    reason = reason or match_stream(body, entry.file_size)
                    if reason:
                        raise ScanFailure(f"{entry_label} matched {reason}")
                    # Test the format, not the extension. This also finds empty and prefixed ZIPs.
                    if entry.file_size >= 22 and zipfile.is_zipfile(body):
                        inspect_archive(body, entry_label, depth + 1)
                    elif kind in ARCHIVES:
                        raise ScanFailure(f"{entry_label} is not a readable archive")

    if found := match(cert_info):
        raise ScanFailure(f"{label} metadata matched {found}")
    if archive_required:
        # Velopack Setup.exe is a PE with an appended ZIP (its bundled full nupkg).
        # zipfile adjusts for the PE prefix. Fail closed if the installer layout changes.
        inspect_archive(path, label, 0)
    with Path(path).open("rb") as body:
        if found := match_stream(body, os.fstat(body.fileno()).st_size):
            raise ScanFailure(f"{label} raw bytes matched {found}")


def apk_main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk", type=Path)
    parser.add_argument("--certificate-stdin", action="store_true", help="scan apksigner's certificate output too")
    args = parser.parse_args()
    try:
        cert = sys.stdin.buffer.read() if args.certificate_stdin else b""
        scan(args.apk, tuple(needles()), cert)
    except (OSError, RuntimeError, ValueError, zipfile.BadZipFile) as error:
        # A failed path or malformed archive may include secrets in its exception text.
        detail = str(error) if isinstance(error, ScanFailure) else type(error).__name__
        parser.exit(1, f"Release privacy scan failed: {detail}\n")
    print("APK privacy scan passed.")
