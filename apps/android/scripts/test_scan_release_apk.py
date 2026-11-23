#!/usr/bin/env python3
"""Regression tests for the read-only scan used before publishing an APK."""

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("scanner", Path(__file__).with_name("scan-release-apk.py"))
scanner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scanner)


class ScannerTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.apk = self.root / "test.apk"

    def write(self, **entries):
        with zipfile.ZipFile(self.apk, "w", zipfile.ZIP_DEFLATED) as archive:
            for name, data in entries.items():
                archive.writestr(name, data)

    def test_remapped_paths_are_safe(self):
        self.write(**{"lib/arm64-v8a/libtodo_ffi.so": b"\x00/src/crates/test.rs\x00/cargo/registry/example.rs\x00/rustc/lib.rs"})
        scanner.scan(self.apk, tuple(scanner.needles()))

    def test_dex_unicode_and_native_path_at_chunk_boundary_are_rejected(self):
        self.write(**{"classes.dex": r"C:\Users\example\work".encode("utf-16le")})
        with self.assertRaisesRegex(ValueError, r"APK entry 1 \(\.dex\) matched user directory prefix"):
            scanner.scan(self.apk, tuple(scanner.needles()))
        self.write(**{"lib/arm64-v8a/libtodo_ffi.so": b"\x00" * (1024 * 1024 - 3) + b"/home/example/dev"})
        with self.assertRaisesRegex(ValueError, r"APK entry 1 \(\.so\) matched user directory prefix"):
            scanner.scan(self.apk, tuple(scanner.needles()))

    def test_username_matches_an_account_not_a_suffix_in_library_code(self):
        with patch.dict("os.environ", {"USERNAME": "example-account"}):
            self.write(**{"classes.dex": b"ApiExample-Account"})
            scanner.scan(self.apk, tuple(scanner.needles()))
            self.write(**{"classes.dex": b"author:example-account@todo.example.com"})
            with self.assertRaisesRegex(ValueError, r"matched local user name$"):
                scanner.scan(self.apk, tuple(scanner.needles()))

    def test_denylist_and_username_are_checked_without_printing_values(self):
        deny = self.root / "deny.txt"
        deny.write_text("# comment\n\nexample-private-value\n", encoding="utf-8")
        self.write(**{"classes.dex": b"EXAMPLE-PRIVATE-VALUE"})
        with patch.dict("os.environ", {"TODO_ANDROID_DENYLIST": str(deny), "USERNAME": "example-account"}):
            with self.assertRaisesRegex(ValueError, r"matched private denylist$"):
                scanner.scan(self.apk, tuple(scanner.needles()))
            with self.assertRaisesRegex(ValueError, r"APK metadata matched local user name$"):
                scanner.scan(self.apk, tuple(scanner.needles()), b"CN=Example-Account")


if __name__ == "__main__":
    unittest.main()
