#!/usr/bin/env python3
"""Test the actual publisher's OpenSSL Ed25519 PEM path with throwaway keys."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SIGNER = Path(__file__).with_name("sign-update-feed.py")
TAG = "v0.1.0-build.9"


class SignerTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        self.pem = root / "example private key.pem"
        self.pub = root / "public.pem"
        self.keys = root / "keys.txt"
        self.feed = root / "releases.win.json"
        self.proof = root / "desktop-update.sig.json"
        self.feed.write_bytes(b'{"Assets": []}\r\n')
        subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", self.pem], check=True)
        subprocess.run(["openssl", "pkey", "-in", self.pem, "-pubout", "-out", self.pub], check=True)
        public_pin = subprocess.check_output([sys.executable, "-B", SIGNER, "export-public", self.pub])
        self.keys.write_bytes(b"# Test key list\n" + public_pin)

    def invoke(self, action, tag=TAG, succeeds=True):
        env = dict(os.environ, TODO_DESKTOP_FEED_KEY=str(self.pem))
        result = subprocess.run([sys.executable, "-B", SIGNER, action, tag, self.feed, self.proof,
                                 "--keys", self.keys], env=env, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0 if succeeds else 1, result.stderr)
        self.assertNotIn(b"PRIVATE KEY", result.stderr + result.stdout)
        self.assertNotIn(self.pem.name.encode(), result.stderr + result.stdout)

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

    def test_refuses_an_unpinned_private_key(self):
        self.keys.write_text("11" * 32, encoding="utf-8")
        self.invoke("sign", succeeds=False)
        self.assertFalse(self.proof.exists())


if __name__ == "__main__":
    unittest.main()
