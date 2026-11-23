#!/usr/bin/env python3
"""Exercise the desktop scan with installer-shaped and nested package fixtures."""

from hashlib import sha256
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile


SCANNER = Path(__file__).with_name("scan-release-desktop.py")
SPEC = importlib.util.spec_from_file_location("scrubber", SCANNER.with_name("scrub-velopack-templates.py"))
SCRUBBER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCRUBBER)
PRIVATE = "example-private-build-value"


def archive(entries):
    data = io.BytesIO()
    with zipfile.ZipFile(data, "w", zipfile.ZIP_DEFLATED) as bundle:
        for name, content in entries.items():
            bundle.writestr(name, content)
    return data.getvalue()


class TemplateScrubTest(unittest.TestCase):
    def test_remap_is_fixed_width_idempotent_and_rejects_unrecognized_templates(self):
        original = b"MZ\0" + SCRUBBER.UPSTREAM + b"\\.cargo\\source.rs\0"
        expected = sha256(original).hexdigest()
        clean = SCRUBBER.remap(original, expected)
        self.assertEqual(len(original), len(clean))
        self.assertNotIn(SCRUBBER.UPSTREAM, clean)
        self.assertEqual(SCRUBBER.remap(clean, expected), clean)
        with self.assertRaisesRegex(ValueError, "Unrecognized Velopack template"):
            SCRUBBER.remap(clean + b"X", expected)


class DesktopScanTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.denylist = self.root / "deny.txt"
        self.denylist.write_text(f"# Private scan values\n\n{PRIVATE}\n", encoding="utf-8")
        self.setup = self.root / "todo-desktop-win-Setup.exe"
        self.package = self.root / "todo-desktop-0.1.0-build.9-full.nupkg"
        self.package.write_bytes(archive({"lib/app/todo-desktop.exe": b"MZ\0/cargo/src/lib.rs\0/src/main.rs"}))
        self.setup.write_bytes(b"MZ\0" + self.package.read_bytes())
        (self.root / "releases.win.json").write_text('{"Assets":[]}', encoding="utf-8")

    def scan(self, expected=None, *, denylist=True, notes="Public notes", extra_env=None):
        env = dict(os.environ)
        env.pop("TODO_DESKTOP_DENYLIST", None)
        if denylist:
            env["TODO_DESKTOP_DENYLIST"] = str(self.denylist)
        env.update(extra_env or {})
        result = subprocess.run([sys.executable, "-B", str(SCANNER), str(self.root), "--require-denylist", "--notes-stdin"],
                                input=notes, env=env, capture_output=True, text=True, check=False)
        if expected is None:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertEqual(result.returncode, 1, result)
            self.assertIn(expected, result.stderr)
        self.assertNotIn(PRIVATE, result.stderr)
        self.assertNotIn(str(self.root), result.stderr)

    def test_notes_and_native_toolchain_paths_are_scanned(self):
        self.scan("Release notes matched private denylist", notes=PRIVATE)
        for name in ("VCToolsInstallDir", "WindowsSdkDir"):
            directory = self.root / name
            self.scan(f"Release notes matched {name}", notes=str(directory), extra_env={name: str(directory)})

    def test_remapped_build_passes(self):
        self.scan()

    def test_nested_package_with_utf16_denylist_match_fails(self):
        payload = archive({"todo-desktop.exe": PRIVATE.upper().encode("utf-16le")})
        self.setup.write_bytes(b"MZ\0" + archive({"resources/data": payload}))
        self.scan("Windows installer entry 1 (other) entry 1 (.exe) matched private denylist")

    def test_raw_installer_stub_is_scanned(self):
        self.setup.write_bytes(b"MZ\0" + PRIVATE.encode() + b"\0" + self.package.read_bytes())
        self.scan("Windows installer raw bytes matched private denylist")

    def test_pdb_and_malformed_setup_are_rejected(self):
        self.package.write_bytes(archive({"lib/app/todo-desktop.PDB": b"symbols"}))
        self.scan("contains a debug symbols file")
        self.package.write_bytes(archive({"lib/app/todo-desktop.exe": b"safe"}))
        self.setup.write_bytes(b"MZ\0no archive")
        self.scan("BadZipFile")

    def test_denylist_is_required_for_publish(self):
        self.scan("private denylist is required", denylist=False)
        self.denylist.write_text("# No active patterns\n", encoding="utf-8")
        self.scan("denylist contains no active patterns")


if __name__ == "__main__":
    unittest.main()
