#!/usr/bin/env python3
"""Sign or verify the exact Velopack feed bytes with an offline Ed25519 key."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


DOMAIN = b"todo-desktop-update-v1\n"
ED25519_SPKI = bytes.fromhex("302a300506032b6570032100")
MAX_FEED = 1024 * 1024
MAX_PROOF = 1024
VERSION_TAG = re.compile(r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)-build\.[1-9][0-9]*")
DEFAULT_KEYS = Path(__file__).resolve().parents[1] / "resources" / "update-keys.txt"


def openssl(*args):
    # Don't forward OpenSSL diagnostics; they may include paths or OpenSSL store configuration.
    return subprocess.run(["openssl", *args], stdin=subprocess.DEVNULL, capture_output=True, check=False)


def public_key(der):
    if not der.startswith(ED25519_SPKI) or len(der) != len(ED25519_SPKI) + 32:
        raise ValueError("expected an Ed25519 key")
    return der[len(ED25519_SPKI):]


def keys(path):
    values = [line.strip() for line in path.read_text(encoding="utf-8").splitlines()]
    pins = [value for value in values if value and not value.startswith("#")]
    if not pins or any(not re.fullmatch(r"[0-9a-fA-F]{64}", pin) for pin in pins):
        raise ValueError("invalid public key list")
    return [bytes.fromhex(pin) for pin in pins]


def message(tag, manifest):
    if not VERSION_TAG.fullmatch(tag) or manifest.stat().st_size > MAX_FEED:
        raise ValueError("invalid feed or tag")
    return DOMAIN + tag.encode("ascii") + b"\n" + manifest.read_bytes()


def verify(tag, manifest, proof_file, pins):
    if proof_file.stat().st_size > MAX_PROOF:
        raise ValueError("invalid signature envelope")
    proof = json.loads(proof_file.read_bytes())
    if (not isinstance(proof, dict) or set(proof) != {"tag", "signature"} or proof["tag"] != tag
            or not isinstance(proof["signature"], str) or not re.fullmatch(r"[0-9a-fA-F]{128}", proof["signature"])):
        raise ValueError("invalid signature envelope")
    with tempfile.TemporaryDirectory(dir=manifest.parent) as dir_name:
        work = Path(dir_name)
        data = work / "input"
        data.write_bytes(message(tag, manifest))
        signature = work / "signature"
        signature.write_bytes(bytes.fromhex(proof["signature"]))
        pin = work / "pin.der"
        for key in pins:
            pin.write_bytes(ED25519_SPKI + key)
            if openssl("pkeyutl", "-verify", "-rawin", "-pubin", "-keyform", "DER", "-inkey", pin,
                       "-in", data, "-sigfile", signature).returncode == 0:
                return
    raise ValueError("no trusted key verified the signature")


def sign(tag, manifest, proof_file, pins, private_path):
    if not private_path:
        raise ValueError("missing private key path")
    der = openssl("pkey", "-in", private_path, "-pubout", "-outform", "DER")
    if der.returncode or public_key(der.stdout) not in pins:
        raise ValueError("the signing key does not match a trusted public key")
    with tempfile.TemporaryDirectory(dir=manifest.parent) as dir_name:
        data = Path(dir_name) / "input"
        data.write_bytes(message(tag, manifest))
        result = openssl("pkeyutl", "-sign", "-rawin", "-inkey", private_path, "-in", data)
        if result.returncode or len(result.stdout) != 64:
            raise ValueError("could not sign the feed")
    with proof_file.open("x", encoding="utf-8", newline="\n") as file:
        json.dump({"tag": tag, "signature": result.stdout.hex()}, file)
        file.write("\n")
    verify(tag, manifest, proof_file, pins)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="action", required=True)
    for action in ("sign", "verify"):
        cmd = subcommands.add_parser(action)
        cmd.add_argument("tag")
        cmd.add_argument("manifest", type=Path)
        cmd.add_argument("proof", type=Path)
        cmd.add_argument("--keys", type=Path, default=DEFAULT_KEYS)
    export = subcommands.add_parser("export-public", help="write a raw hex pin from a public PEM")
    export.add_argument("public_pem", type=Path)
    args = parser.parse_args()
    try:
        if args.action == "export-public":
            result = openssl("pkey", "-pubin", "-in", args.public_pem, "-pubout", "-outform", "DER")
            if result.returncode:
                raise ValueError("invalid public PEM")
            print(public_key(result.stdout).hex())
        elif args.action == "sign":
            sign(args.tag, args.manifest, args.proof, keys(args.keys), os.getenv("TODO_DESKTOP_FEED_KEY"))
        else:
            verify(args.tag, args.manifest, args.proof, keys(args.keys))
    except (OSError, ValueError):
        parser.exit(1, "Desktop feed signing or verification failed. Check the tag, feed, OpenSSL and trusted Ed25519 keys.\n")


if __name__ == "__main__":
    main()
