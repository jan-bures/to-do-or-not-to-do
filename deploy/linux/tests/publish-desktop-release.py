#!/usr/bin/env python3
"""Exercise the publisher without touching a running server or using sudo."""

import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location("publisher", Path(__file__).parents[1] / "publish-desktop-release.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


def release(path, height, content):
    version = f"0.1.0-build.{height}"
    name = f"todo-desktop-{version}-full.nupkg"
    (path / name).write_bytes(content)
    (path / publisher.SETUP).write_bytes(b"MZfake")
    (path / publisher.INDEX).write_text(json.dumps({"Assets": [{
        "Version": version, "PackageId": "todo-desktop", "Type": "Full", "FileName": name,
        "SHA256": hashlib.sha256(content).hexdigest(), "Size": len(content),
    }]}))
    return name


class PublishingTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        self.source = root / "upload"
        self.dest = root / "feed"
        self.source.mkdir()
        self.dest.mkdir()

    def test_publish_and_keep_old_downloads(self):
        old = release(self.source, 9, b"old")
        publisher.publish(self.source, self.dest)
        new = release(self.source, 10, b"new")
        publisher.publish(self.source, self.dest)
        self.assertEqual((self.dest / publisher.INDEX).read_bytes(), (self.source / publisher.INDEX).read_bytes())
        self.assertEqual((self.dest / old).read_bytes(), b"old")
        self.assertEqual((self.dest / new).read_bytes(), b"new")
        self.assertEqual((self.dest / publisher.SETUP).read_bytes(), b"MZfake")
        with self.assertRaisesRegex(ValueError, "must be newer"):
            publisher.publish(self.source, self.dest)

    def test_corrupt_upload_does_not_replace_live_feed(self):
        release(self.source, 9, b"old")
        publisher.publish(self.source, self.dest)
        previous = (self.dest / publisher.INDEX).read_bytes()
        new = release(self.source, 10, b"new")
        (self.source / new).write_bytes(b"bad")
        with self.assertRaisesRegex(ValueError, "hash or size"):
            publisher.publish(self.source, self.dest)
        self.assertFalse((self.dest / new).exists())
        self.assertEqual((self.dest / publisher.INDEX).read_bytes(), previous)

    def test_package_names_are_not_paths(self):
        release(self.source, 9, b"old")
        index = self.source / publisher.INDEX
        manifest = json.loads(index.read_bytes())
        manifest["Assets"][0]["FileName"] = "../package.nupkg"
        index.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "package name"):
            publisher.publish(self.source, self.dest)
        self.assertFalse((self.dest / publisher.INDEX).exists())


if __name__ == "__main__":
    unittest.main()
