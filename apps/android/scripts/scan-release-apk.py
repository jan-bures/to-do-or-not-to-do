#!/usr/bin/env python3
"""Refuse to release an APK that contains local paths or other denylisted bytes."""

import argparse
import getpass
import os
from pathlib import Path, PurePosixPath
import re
import sys
import zipfile


def spellings(value):
    return {value, value.replace("\\", "/"), value.replace("/", "\\"), value.replace("\\", "\\\\")}


def needles():
    paths = {str(Path.home()), os.getenv("USERPROFILE", ""), os.getenv("HOME", "")}
    values = {
        "home path": {form for path in paths if path for form in spellings(path)},
        "local user name": {getpass.getuser(), os.getenv("USERNAME", ""), os.getenv("USER", "")},
        "user directory prefix": spellings(r"C:\Users") | {"/home/", "/Users/", "\\home\\", "\\Users\\"},
    }
    denylist = os.getenv("TODO_ANDROID_DENYLIST")
    if denylist:
        lines = Path(denylist).read_text(encoding="utf-8-sig").splitlines()
        values["private denylist"] = {line.strip() for line in lines if line.strip() and not line.lstrip().startswith("#")}
    for reason, strings in values.items():
        for text in sorted(strings - {""}):
            for encoding in ("utf-8", "utf-16le", "utf-16be"):
                raw = text.lower().encode(encoding)
                # An account name is a token, not the suffix of e.g. ApiCallRunner.
                token = None
                if reason == "local user name":
                    word = b"[a-z0-9_]"
                    if encoding == "utf-16le":
                        word += b"\x00"
                    elif encoding == "utf-16be":
                        word = b"\x00" + word
                    token = re.compile(b"(?<!" + word + b")" + re.escape(raw) + b"(?!" + word + b")")
                yield raw, reason, token


def scan(apk, patterns, cert_info=b""):
    """Stream every decompressed entry, including dex and the native library. Never write entries to disk."""
    def match(data, has_prefix=False, has_suffix=False):
        data = data.lower()
        for word, reason, token in patterns:
            if word not in data:
                continue
            if token is None or any((not has_prefix or hit.start() >= 2) and (not has_suffix or hit.end() <= len(data) - 2)
                                    for hit in token.finditer(data)):
                return reason
        return None

    overlap = max(len(word) for word, _, _ in patterns) + 4
    with zipfile.ZipFile(apk) as archive:
        if found := match(archive.comment + cert_info):
            raise ValueError(f"APK metadata matched {found}")
        for index, entry in enumerate(archive.infolist(), start=1):
            # Don't print entry names: one could itself include a private value.
            kind = PurePosixPath(entry.filename).suffix
            kind = kind if kind in {".dex", ".so", ".xml", ".arsc"} else "other"
            reason = match(entry.filename.encode("utf-8")) or match(entry.comment) or match(entry.extra)
            with archive.open(entry) as body:
                previous = b""
                remaining = entry.file_size
                seen = 0
                while reason is None and (chunk := body.read(1024 * 1024)):
                    remaining -= len(chunk)
                    data = previous + chunk
                    reason = match(data, has_prefix=seen > len(previous), has_suffix=remaining > 0)
                    seen += len(chunk)
                    previous = data[-overlap:]
            if reason:
                raise ValueError(f"APK entry {index} ({kind}) matched {reason}")


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
