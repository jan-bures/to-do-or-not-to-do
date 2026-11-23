#!/usr/bin/env python3
"""Reject local or denied bytes in the Android release before publishing."""

from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "scripts"))
from release_privacy import apk_main

if __name__ == "__main__":
    apk_main()
