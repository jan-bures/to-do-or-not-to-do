#!/usr/bin/env python3
"""Regression tests for the read-only scan used before publishing an APK."""

import importlib.util
import io
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

    def test_workspace_outside_home_matches_either_slash_style_and_case(self):
        workspace = self.root / "separate" / "workspace"
        workspace.mkdir(parents=True)
        with patch.object(scanner, "command_value", return_value=""), patch("tempfile.gettempdir", return_value=str(self.root / "temp")):
            patterns = tuple(scanner.needles(workspace))
        for path in scanner.spellings(str(workspace)):
            self.write(**{"lib/arm64-v8a/libtodo_ffi.so": (path.swapcase() + "/src/main.rs").encode("utf-8")})
            with self.assertRaisesRegex(ValueError, r"matched build path$"):
                scanner.scan(self.apk, patterns)

    def test_git_identity_hostname_and_toolchain_paths(self):
        values = {"user.name": "Test Build Name", "user.email": "builder@todo.example.com", "sysroot": str(self.root / "toolchain")}
        with patch.object(scanner, "command_value", side_effect=lambda root, args, **kwargs: values[args[-1]]), \
             patch("socket.gethostname", return_value="test-builder-machine"), \
             patch.dict("os.environ", {"GRADLE_USER_HOME": str(self.root / "gradle-cache")}):
            patterns = tuple(scanner.needles())
        for value in (*values.values(), "test-builder-machine", str(self.root / "gradle-cache")):
            self.write(**{"classes.dex": value.upper().encode("utf-16le")})
            with self.assertRaisesRegex(ValueError, r"APK entry 1 \(\.dex\) matched"):
                scanner.scan(self.apk, patterns)

    def test_mixed_case_unicode_identity_matches_in_binary(self):
        with patch.object(scanner, "command_value", side_effect=lambda root, args, **kwargs: "Äntä Örn" if args[-1] == "user.name" else ""):
            patterns = tuple(scanner.needles())
        for encoding in ("utf-8", "utf-16le", "utf-16be"):
            self.write(**{"classes.dex": b"\x00" + "äNTÄ öRN".encode(encoding)})
            with self.assertRaisesRegex(ValueError, r"matched git author name$"):
                scanner.scan(self.apk, patterns)

    def test_nested_jar_is_detected_without_relying_on_its_name(self):
        jar = io.BytesIO()
        with zipfile.ZipFile(jar, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("classes.dex", b"x" * 200 + b"/home/example/build")
        for name in ("assets/resources.jar", "assets/blob"):
            self.write(**{name: jar.getvalue()})
            with self.assertRaisesRegex(ValueError, r"APK entry 1 .* entry 1 \(\.dex\) matched user directory prefix"):
                scanner.scan(self.apk, tuple(scanner.needles()))

    def test_appended_bytes_are_scanned(self):
        self.write(**{"classes.dex": b"safe"})
        with self.apk.open("ab") as file:
            file.write(b"\x00private bytes: /Users/example/hidden\x00")
        with self.assertRaisesRegex(ValueError, r"APK raw bytes matched user directory prefix"):
            scanner.scan(self.apk, tuple(scanner.needles()))

    def test_empty_or_comment_only_denylist_is_invalid(self):
        deny = self.root / "deny.txt"
        for value in ("", "\n # comment\n"):
            deny.write_text(value, encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "no active patterns"):
                scanner.denylist_values(deny)

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
