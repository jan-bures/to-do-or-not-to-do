#!/usr/bin/env python3
"""Fetch the F2 fonts (OFL) and cut the static instances the desktop app loads.

Stopgap until the shared `assets/fonts` directory lands with the design tokens. The app looks for fonts in
`--fonts-dir`, then `TODO_FONTS_DIR`, then an `assets/fonts` directory next to or above the executable, and
finally in `apps/desktop/target/fonts`, which this script fills (and git ignores).

Usage: python apps/desktop/scripts/fetch_fonts.py [out_dir]
Needs fontTools (`pip install fonttools==4.65.0`).
"""
import io
import os
import sys
import urllib.request

from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

# Pinned google/fonts commit, so every machine (and CI) cuts identical fonts.
FONTS_REV = "06bfe0d62a98375cb09c942fa18e68bd32cff057"
BASE = f"https://raw.githubusercontent.com/google/fonts/{FONTS_REV}/ofl/"
SOURCES = {
    "fraunces": "fraunces/Fraunces%5BSOFT,WONK,opsz,wght%5D.ttf",
    "fraunces-italic": "fraunces/Fraunces-Italic%5BSOFT,WONK,opsz,wght%5D.ttf",
    "schibsted": "schibstedgrotesk/SchibstedGrotesk%5Bwght%5D.ttf",
    "geist-mono": "geistmono/GeistMono%5Bwght%5D.ttf",
}
LICENSES = {
    "Fraunces-OFL.txt": "fraunces/OFL.txt",
    "SchibstedGrotesk-OFL.txt": "schibstedgrotesk/OFL.txt",
    "GeistMono-OFL.txt": "geistmono/OFL.txt",
}
# (source, output file, axis location, style name, italic). The display cut matches the F2 room title
# (opsz 144, SOFT 100, WONK 0, weight 360); the italic matches the "Saturday" eyebrow (opsz 36, SOFT 100).
INSTANCES = [
    ("fraunces", "Fraunces-Display.ttf", {"opsz": 144, "SOFT": 100, "WONK": 0, "wght": 360}, "Regular", False),
    ("fraunces-italic", "Fraunces-Italic.ttf", {"opsz": 36, "SOFT": 100, "WONK": 1, "wght": 400}, "Italic", True),
    ("schibsted", "SchibstedGrotesk-Regular.ttf", {"wght": 400}, "Regular", False),
    ("schibsted", "SchibstedGrotesk-Medium.ttf", {"wght": 500}, "Medium", False),
    ("schibsted", "SchibstedGrotesk-SemiBold.ttf", {"wght": 600}, "SemiBold", False),
    ("schibsted", "SchibstedGrotesk-Bold.ttf", {"wght": 700}, "Bold", False),
    ("geist-mono", "GeistMono-Regular.ttf", {"wght": 400}, "Regular", False),
    ("geist-mono", "GeistMono-Medium.ttf", {"wght": 500}, "Medium", False),
    ("geist-mono", "GeistMono-SemiBold.ttf", {"wght": 600}, "SemiBold", False),
]
FAMILY = {"fraunces": "Fraunces", "fraunces-italic": "Fraunces", "schibsted": "Schibsted Grotesk", "geist-mono": "Geist Mono"}


def fetch(path: str) -> bytes:
    req = urllib.request.Request(BASE + path, headers={"User-Agent": "fetch-fonts"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.read()


def set_names(font: TTFont, family: str, style: str, weight: int, italic: bool) -> None:
    """Static names so DirectWrite groups each family and matches by weight and style."""
    name = font["name"]
    full = f"{family} {style}".strip()
    ps = f"{family.replace(' ', '')}-{style.replace(' ', '')}"
    for rec in list(name.names):
        if rec.nameID in (16, 17, 21, 22, 25):
            name.removeNames(nameID=rec.nameID)
    legacy_style = "Italic" if italic else "Regular"
    legacy_family = family if style in ("Regular", "Italic") else f"{family} {style}"
    for nid, val in ((1, legacy_family), (2, legacy_style), (4, full), (6, ps), (16, family), (17, style)):
        name.setName(val, nid, 3, 1, 0x409)
    font["OS/2"].usWeightClass = weight
    if italic:
        font["OS/2"].fsSelection = (font["OS/2"].fsSelection | 0x01) & ~0x40
        font["head"].macStyle |= 0x02


def main() -> None:
    here = os.path.dirname(os.path.abspath(__file__))
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "..", "target", "fonts")
    os.makedirs(out, exist_ok=True)
    cache = {}
    for src, file, loc, style, italic in INSTANCES:
        if src not in cache:
            cache[src] = fetch(SOURCES[src])
        # Keep the source's head.modified so repeated runs write identical bytes.
        font = TTFont(io.BytesIO(cache[src]), recalcTimestamp=False)
        axes = {a.axisTag for a in font["fvar"].axes}
        static = instancer.instantiateVariableFont(font, {k: v for k, v in loc.items() if k in axes})
        # DirectWrite names families from STAT's axis values ("Fraunces 144pt SuperSoft") when the table is
        # present; a static instance doesn't need it.
        if "STAT" in static:
            del static["STAT"]
        set_names(static, FAMILY[src], style, int(loc.get("wght", 400)), italic)
        static.save(os.path.join(out, file))
        print("wrote", file)
    for file, path in LICENSES.items():
        with open(os.path.join(out, file), "wb") as f:
            f.write(fetch(path))
    print("fonts in", os.path.abspath(out))


if __name__ == "__main__":
    main()
