#!/usr/bin/env python3
"""Sign or verify a Velopack feed and installer with an offline Ed25519 key."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

DOMAIN = b"todo-desktop-update-v2\n"
ED25519_SPKI = bytes.fromhex("302a300506032b6570032100")
MAX_FEED = 1024 * 1024
MAX_PROOF = 1024
VERSION_TAG = re.compile(r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)-build\.[1-9][0-9]*")
APP = Path(__file__).resolve().parents[1]
DEFAULT_KEYS = APP / "resources" / "update-keys.txt"


def openssl(*args):
    # OpenSSL diagnostics may include private filesystem paths; never forward them.
    return subprocess.run(["openssl", *args], stdin=subprocess.DEVNULL, capture_output=True, check=False)


def public_key(der):
    if not der.startswith(ED25519_SPKI) or len(der) != len(ED25519_SPKI) + 32:
        raise ValueError("expected an Ed25519 key")
    return der[len(ED25519_SPKI):]


def keys(path):
    lines = [line.strip() for line in path.read_text(encoding="utf-8").splitlines()]
    pins = [line for line in lines if line and not line.startswith("#")]
    if not pins or any(not re.fullmatch(r"[0-9a-fA-F]{64}", pin) for pin in pins):
        raise ValueError("invalid public key list")
    return [bytes.fromhex(pin) for pin in pins]


def file_identity(path):
    with path.open("rb") as file:
        digest = hashlib.sha256()
        size = 0
        while chunk := file.read(1024 * 1024):
            size += len(chunk)
            digest.update(chunk)
    return digest.hexdigest(), size


def bounded_read(path, limit):
    with path.open("rb") as file:
        data = file.read(limit + 1)
    if len(data) > limit:
        raise ValueError("oversized feed or proof")
    return data


def message(proof, manifest):
    tag, digest, size = proof["tag"], proof["setup_sha256"], proof["setup_size"]
    if (not isinstance(tag, str) or not VERSION_TAG.fullmatch(tag)
            or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest)
            or type(size) is not int or not 0 < size <= 2**64 - 1):
        raise ValueError("invalid feed or installer identity")
    return DOMAIN + f"{tag}\n{digest}\n{size}\n".encode("ascii") + bounded_read(manifest, MAX_FEED)


def unique_object(pairs):
    result = dict(pairs)
    if len(result) != len(pairs):
        raise ValueError("duplicate proof field")
    return result


def verify(tag, manifest, proof_file, pins):
    proof = json.loads(bounded_read(proof_file, MAX_PROOF).decode("utf-8"), object_pairs_hook=unique_object)
    if (not isinstance(proof, dict) or set(proof) != {"tag", "signature", "setup_size", "setup_sha256"}
            or (tag is not None and proof["tag"] != tag)
            or not isinstance(proof["signature"], str) or not re.fullmatch(r"[0-9a-fA-F]{128}", proof["signature"])):
        raise ValueError("invalid signature envelope")
    with tempfile.TemporaryDirectory() as dir_name:
        work = Path(dir_name)
        data = work / "input"
        data.write_bytes(message(proof, manifest))
        signature = work / "signature"
        signature.write_bytes(bytes.fromhex(proof["signature"]))
        pin = work / "pin.der"
        for key in pins:
            pin.write_bytes(ED25519_SPKI + key)
            if openssl("pkeyutl", "-verify", "-rawin", "-pubin", "-keyform", "DER", "-inkey", pin,
                       "-in", data, "-sigfile", signature).returncode == 0:
                return proof
    raise ValueError("no trusted key verified the signature")


def resolved_key(private_path, *, test_key=False):
    if not private_path:
        raise ValueError("missing private key path")
    path = Path(private_path).resolve(strict=True)
    if not path.is_file() or (not test_key and path.is_relative_to(APP.parents[1].resolve(strict=True))):
        raise ValueError("private key must live outside the repository")
    return path


def sign(tag, manifest, proof_file, setup, pins, private_path):
    der = openssl("pkey", "-in", private_path, "-pubout", "-outform", "DER")
    if der.returncode or public_key(der.stdout) not in pins:
        raise ValueError("the signing key does not match a trusted public key")
    digest, size = file_identity(setup)
    proof = {"tag": tag, "setup_sha256": digest, "setup_size": size}
    with tempfile.TemporaryDirectory() as dir_name:
        data = Path(dir_name) / "input"
        data.write_bytes(message(proof, manifest))
        result = openssl("pkeyutl", "-sign", "-rawin", "-inkey", private_path, "-in", data)
        if result.returncode or len(result.stdout) != 64:
            raise ValueError("could not sign the feed")
    with proof_file.open("x", encoding="utf-8", newline="\n") as file:
        json.dump(dict(proof, signature=result.stdout.hex()), file)
        file.write("\n")
    verify_setup(setup, proof_file, manifest, pins, tag)


def verify_setup(setup, proof_file, manifest, pins, tag=None):
    proof = verify(tag, manifest, proof_file, pins)
    if file_identity(setup) != (proof["setup_sha256"], proof["setup_size"]):
        raise ValueError("installer bytes do not match the signed proof")
    return proof["tag"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="action", required=True)
    for action in ("sign", "verify"):
        cmd = subcommands.add_parser(action)
        cmd.add_argument("tag")
        cmd.add_argument("manifest", type=Path)
        cmd.add_argument("proof", type=Path)
        cmd.add_argument("--keys", type=Path, default=DEFAULT_KEYS)
        if action == "sign":
            cmd.add_argument("--setup", type=Path, required=True)
            cmd.add_argument("--allow-test-key-in-checkout", action="store_true", help=argparse.SUPPRESS)
    setup = subcommands.add_parser("verify-setup", help="verify the installer against the checked-in public key list")
    setup.add_argument("installer", type=Path)
    setup.add_argument("proof", type=Path)
    setup.add_argument("manifest", type=Path)
    subcommands.add_parser("check-key", help="require the resolved private key to live outside the repository")
    export = subcommands.add_parser("export-public", help="write a raw hex pin from a public PEM")
    export.add_argument("public_pem", type=Path)
    args = parser.parse_args()
    try:
        private = os.getenv("TODO_DESKTOP_FEED_KEY")
        if args.action == "export-public":
            result = openssl("pkey", "-pubin", "-in", args.public_pem, "-pubout", "-outform", "DER")
            if result.returncode:
                raise ValueError("invalid public PEM")
            print(public_key(result.stdout).hex())
        elif args.action == "check-key":
            resolved_key(private)
        elif args.action == "sign":
            path = resolved_key(private, test_key=args.allow_test_key_in_checkout)
            sign(args.tag, args.manifest, args.proof, args.setup, keys(args.keys), path)
        elif args.action == "verify-setup":
            tag = verify_setup(args.installer, args.proof, args.manifest, keys(DEFAULT_KEYS))
            print(f"Desktop installer verified for {tag}.")
        else:
            verify(args.tag, args.manifest, args.proof, keys(args.keys))
    except (OSError, ValueError, RuntimeError):
        parser.exit(1, "Desktop feed signing or verification failed. Check the tag, feed, installer, OpenSSL and trusted Ed25519 keys. Keep the signing key outside the repository.\n")


if __name__ == "__main__":
    main()
