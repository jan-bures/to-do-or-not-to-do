#!/usr/bin/env python3
"""Refuse to release an APK that contains local paths or other denylisted bytes."""

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


REPO_ROOT = Path(__file__).resolve().parents[3]
CHUNK_SIZE = 1024 * 1024
MAX_NESTING = 8
MAX_EXPANDED = 512 * 1024 * 1024


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


def path_needles(root):
    home = Path.home()
    paths = {
        root, home / ".cargo", home / ".rustup", home / ".gradle", Path(tempfile.gettempdir()),
        Path(command_value(root, ["rustc", "--print", "sysroot"])),
    }
    for key in ("CARGO_HOME", "RUSTUP_HOME", "GRADLE_USER_HOME", "ANDROID_HOME", "ANDROID_SDK_ROOT",
                "ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "TMP", "TEMP", "TMPDIR"):
        if value := os.getenv(key):
            paths.add(Path(value))
    # Gradle can get its SDK from local.properties even when the environment is empty.
    properties = root / "apps" / "android" / "local.properties"
    if properties.is_file():
        for line in properties.read_text(encoding="utf-8-sig").splitlines():
            if found := re.fullmatch(r"\s*(?:sdk|ndk)\.dir\s*[=:]\s*(.+?)\s*", line):
                paths.add(Path(re.sub(r"\\([\\ :])", r"\1", found[1])))
    values = set()
    for path in paths:
        path = path if path.is_absolute() else root / path
        for absolute in (path.absolute(), path.resolve()):
            if absolute != Path(absolute.anchor):
                values.update(spellings(str(absolute)))
    return values


def denylist_values(path):
    lines = Path(path).read_text(encoding="utf-8-sig").splitlines()
    values = {line.strip() for line in lines if line.strip() and not line.lstrip().startswith("#")}
    if not values:
        raise ValueError("denylist contains no active patterns")
    return values


def needles(root=REPO_ROOT):
    paths = {str(Path.home()), os.getenv("USERPROFILE", ""), os.getenv("HOME", "")}
    values = {
        "home path": {form for path in paths if path for form in spellings(path)},
        "local user name": {getpass.getuser(), os.getenv("USERNAME", ""), os.getenv("USER", "")},
        "user directory prefix": spellings(r"C:\Users") | spellings("/home/") | spellings("/Users/"),
        "build path": path_needles(root),
        "git author name": {command_value(root, ["git", "config", "--get", "user.name"], optional=True)},
        "git author email": {command_value(root, ["git", "config", "--get", "user.email"], optional=True)},
        "machine hostname": {socket.gethostname(), os.getenv("COMPUTERNAME", ""), os.getenv("HOSTNAME", "")},
    }
    if denylist := os.getenv("TODO_ANDROID_DENYLIST"):
        values["private denylist"] = denylist_values(denylist)
    for reason, strings in values.items():
        for text in sorted({value.lower() for value in strings if value}):
            for encoding in ("utf-8", "utf-16le", "utf-16be"):
                yield pattern(text, encoding, reason)


def pattern(text, encoding, reason):
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
    bounded = reason in {"local user name", "git author name", "machine hostname"}
    if bounded:
        word = b"[a-z0-9_]"
        if encoding == "utf-16le":
            word += b"\x00"
        elif encoding == "utf-16be":
            word = b"\x00" + word
        expression = b"(?<!" + word + b")" + expression + b"(?!" + word + b")"
    return (raw if text.isascii() else None), reason, (re.compile(expression) if bounded or not text.isascii() else None), width


def scan(apk, patterns, cert_info=b""):
    """Cover raw bytes, expanded entries, and nested archives without extracting paths to disk."""
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
            raise ValueError("APK has too many nested archives")
        with zipfile.ZipFile(source) as archive:
            if found := match(archive.comment):
                raise ValueError(f"{label} metadata matched {found}")
            for index, entry in enumerate(archive.infolist(), start=1):
                # Don't print entry names: one could itself include a private value.
                kind = PurePosixPath(entry.filename).suffix.lower()
                kind = kind if kind in {".dex", ".so", ".xml", ".arsc", ".zip", ".jar"} else "other"
                entry_label = f"{label} entry {index} ({kind})"
                expanded += entry.file_size
                if expanded > MAX_EXPANDED:
                    raise ValueError("APK expanded entries exceed the scan limit")
                reason = match(entry.filename.encode("utf-8")) or match(entry.comment) or match(entry.extra)
                with archive.open(entry) as body:
                    reason = reason or match_stream(body, entry.file_size)
                    if reason:
                        raise ValueError(f"{entry_label} matched {reason}")
                    # Test the format, not the extension. This also finds empty and prefixed ZIPs.
                    if entry.file_size >= 22 and zipfile.is_zipfile(body):
                        inspect_archive(body, entry_label, depth + 1)
                    elif kind in {".jar", ".zip"}:
                        raise ValueError(f"{entry_label} is not a readable archive")

    if found := match(cert_info):
        raise ValueError(f"APK metadata matched {found}")
    inspect_archive(apk, "APK", 0)
    with Path(apk).open("rb") as body:
        if found := match_stream(body, os.fstat(body.fileno()).st_size):
            raise ValueError(f"APK raw bytes matched {found}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk", type=Path)
    parser.add_argument("--certificate-stdin", action="store_true", help="scan apksigner's certificate output too")
    args = parser.parse_args()
    try:
        cert = sys.stdin.buffer.read() if args.certificate_stdin else b""
        scan(args.apk, tuple(needles()), cert)
    except (OSError, RuntimeError, ValueError, zipfile.BadZipFile) as error:
        # A failed path or malformed archive may include secrets in its exception text.
        detail = str(error) if isinstance(error, ValueError) and str(error).startswith("APK ") else type(error).__name__
        parser.exit(1, f"Release privacy scan failed: {detail}\n")
    print("APK privacy scan passed.")


if __name__ == "__main__":
    main()
