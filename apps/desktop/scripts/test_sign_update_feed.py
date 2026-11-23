#!/usr/bin/env python3
"""Test the actual publisher's OpenSSL Ed25519 PEM path with throwaway keys."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SIGNER = Path(__file__).with_name("sign-update-feed.py")
SPEC = importlib.util.spec_from_file_location("release_signer", SIGNER)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
TAG = "v0.1.0-build.9"
FIXTURE = SIGNER.parents[1] / "tests" / "fixtures" / "update-signing"


class SignerTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        self.root = root
        self.pem = root / "example private key.pem"
        self.pub = root / "public.pem"
        self.keys = root / "keys.txt"
        self.feed = root / "releases.win.json"
        self.proof = root / "desktop-update.sig.json"
        self.setup = root / "Setup.exe"
        self.setup.write_bytes(b"MZ sample installer bytes")
        self.feed.write_bytes(b'{"Assets": []}\r\n')
        subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", self.pem], check=True)
        subprocess.run(["openssl", "pkey", "-in", self.pem, "-pubout", "-out", self.pub], check=True)
        public_pin = subprocess.check_output([sys.executable, "-B", SIGNER, "export-public", self.pub])
        self.keys.write_bytes(b"# Test key list\n" + public_pin)

    def invoke(self, action, tag=TAG, succeeds=True):
        env = dict(os.environ, TODO_DESKTOP_FEED_KEY=str(self.pem))
        private_args = ["--setup", self.setup, "--allow-test-key-in-checkout"] if action == "sign" else []
        result = subprocess.run([sys.executable, "-B", SIGNER, action, tag, self.feed, self.proof,
                                 "--keys", self.keys, *private_args], env=env, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0 if succeeds else 1, result.stderr)
        self.assertNotIn(b"PRIVATE KEY", result.stderr + result.stdout)
        self.assertNotIn(self.pem.name.encode(), result.stderr + result.stdout)

    def test_regenerates_the_rust_verification_fixture_byte_for_byte(self):
        # RFC 8410 PKCS#8 encoding with an intentionally public, test-only Ed25519 seed.
        # Never use this key or add its public pin to the app's trusted keys.
        self.pem.write_bytes(bytes.fromhex("302e020100300506032b657004220420") + bytes(range(32)))
        self.keys = FIXTURE / "public-key.txt"
        self.feed = FIXTURE / "releases.win.json"
        self.setup = FIXTURE / "Setup.exe"
        self.invoke("sign")
        self.assertEqual(self.proof.read_bytes(), (FIXTURE / self.proof.name).read_bytes())

    def test_signs_exact_bytes_and_does_not_replace_a_proof(self):
        self.invoke("sign")
        proof = json.loads(self.proof.read_text(encoding="utf-8"))
        self.assertEqual(proof["tag"], TAG)
        self.assertEqual(len(bytes.fromhex(proof["signature"])), 64)
        self.invoke("verify")
        self.invoke("verify", tag="v0.1.0-build.10", succeeds=False)
        self.invoke("sign", succeeds=False)
        self.feed.write_bytes(self.feed.read_bytes().replace(b"\r\n", b"\n"))
        self.invoke("verify", succeeds=False)

    def test_verify_setup_uses_the_pinned_keys_and_rejects_tampering(self):
        self.invoke("sign")
        with patch.object(MODULE, "DEFAULT_KEYS", self.keys), patch.object(sys, "argv", [str(SIGNER), "verify-setup", str(self.setup), str(self.proof), str(self.feed)]):
            MODULE.main()
            original = self.setup.read_bytes()
            for modified in (original + b"!", b"?" + original[1:]):
                self.setup.write_bytes(modified)
                with self.assertRaises(SystemExit) as rejected:
                    MODULE.main()
                self.assertEqual(rejected.exception.code, 1)
            self.setup.write_bytes(original)
            proof = json.loads(self.proof.read_bytes())
            proof["setup_size"] += 1
            self.proof.write_text(json.dumps(proof), encoding="utf-8")
            with self.assertRaises(SystemExit):
                MODULE.main()

    def test_real_key_path_resolves_links_before_the_repository_check(self):
        fake_repo = self.root / "checkout"
        app = fake_repo / "apps" / "desktop"
        app.mkdir(parents=True)
        inner_key = fake_repo / "fixture.pem"
        inner_key.write_bytes(b"test file")
        alias = self.root / "alias"
        if os.name == "nt":
            subprocess.run(["cmd", "/c", "mklink", "/J", alias, fake_repo], capture_output=True, check=True)
            self.addCleanup(alias.rmdir)
        else:
            alias.symlink_to(fake_repo, target_is_directory=True)
        with patch.object(MODULE, "APP", app):
            self.assertEqual(MODULE.resolved_key(self.pem), self.pem.resolve())
            with self.assertRaisesRegex(ValueError, "outside the repository"):
                MODULE.resolved_key(alias / "fixture.pem")

    def test_refuses_an_unpinned_private_key(self):
        self.keys.write_text("11" * 32, encoding="utf-8")
        self.invoke("sign", succeeds=False)
        self.assertFalse(self.proof.exists())


if __name__ == "__main__":
    unittest.main()
