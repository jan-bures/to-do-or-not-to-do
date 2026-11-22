#!/usr/bin/env python3
"""Rebuild the bundled static font instances from the upstream variable fonts.

Usage:
    python build_fonts.py <upstream-dir> [<out-dir>]

<upstream-dir> must contain the files listed in UPSTREAM, downloaded from the sources recorded in README.md.
<out-dir> defaults to the directory this script lives in. Requires fontTools (tested with 4.65).

Each instance pins every variation axis, so GPUI, Compose and browsers all render exactly the same outlines
without needing variable-font support. Each one is then subset to Latin, Latin Extended (full Czech coverage)
and the few symbols the UI uses.
"""
import os
import sys

from fontTools.subset import Options, Subsetter
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

UPSTREAM = {
    'geist': 'Geist[wght].ttf',
    'geist-mono': 'GeistMono[wght].ttf',
}

# (output file, upstream key, family, style name, weight, italic, axis location)
INSTANCES = [
    (f'Geist-{w}.ttf', 'geist', 'Geist', name, w, False, dict(wght=w))
    for w, name in ((400, 'Regular'), (500, 'Medium'), (600, 'SemiBold'))
]
INSTANCES += [
    (f'GeistMono-{w}.ttf', 'geist-mono', 'Geist Mono', name, w, False, dict(wght=w))
    for w, name in ((400, 'Regular'), (500, 'Medium'), (600, 'SemiBold'), (700, 'Bold'))
]

# Google Fonts' "latin" and "latin-ext" ranges, plus Greek pi (the Pi agent's letter mark), arrows, keyboard
# symbols and a few geometric shapes. Code points a font doesn't have are ignored.
UNICODES = (
    [(0x0000, 0x00FF), (0x0100, 0x02BA), (0x02BB, 0x02FF), (0x0304, 0x0304), (0x0308, 0x0308), (0x0329, 0x0329),
     (0x03C0, 0x03C0), (0x1D00, 0x1DBF), (0x1E00, 0x1EFF), (0x2000, 0x206F), (0x20A0, 0x20C0), (0x2113, 0x2113),
     (0x2122, 0x2122), (0x2190, 0x21FF), (0x2212, 0x2215), (0x2300, 0x23FF), (0x25A0, 0x25FF), (0x2713, 0x2713),
     (0x2C60, 0x2C7F), (0xA720, 0xA7FF), (0xFEFF, 0xFEFF), (0xFFFD, 0xFFFD)]
)

RIBBI = {'Regular', 'Italic', 'Bold', 'Bold Italic'}


def set_names(font, family, style, weight, italic):
    name = font['name']
    ps_family = family.replace(' ', '')
    ps_style = style.replace(' ', '')
    full = f'{family} {style}'
    version = name.getDebugName(5) or 'Version 1.000'
    # Drop everything that described the variable font, then write a clean, static set.
    for rec in list(name.names):
        if rec.nameID in (1, 2, 3, 4, 6, 16, 17, 21, 22, 25) or rec.nameID >= 256:
            name.removeNames(nameID=rec.nameID)
    legacy_family, legacy_style = (family, style) if style in RIBBI else (full, 'Italic' if italic else 'Regular')
    for nid, value in ((1, legacy_family), (2, legacy_style), (3, f'{ps_family}-{ps_style};{version}'), (4, full),
                       (6, f'{ps_family}-{ps_style}'), (16, family), (17, style)):
        name.setName(value, nid, 3, 1, 0x409)
    os2 = font['OS/2']
    os2.usWeightClass = weight
    bold = weight >= 700
    regular = not italic and not bold
    os2.fsSelection = (os2.fsSelection & ~0b1100001) | (1 if italic else 0) | (0b100000 if bold else 0) | (
        0b1000000 if regular else 0)
    font['head'].macStyle = (1 if bold else 0) | (2 if italic else 0)
    if italic:
        font['post'].italicAngle = font['post'].italicAngle or -10.0


def build(upstream_dir, out_dir):
    for out, key, family, style, weight, italic, loc in INSTANCES:
        vf = TTFont(os.path.join(upstream_dir, UPSTREAM[key]))
        inst = instantiateVariableFont(vf, loc, updateFontNames=False)
        for table in ('STAT', 'DSIG'):
            if table in inst:
                del inst[table]
        set_names(inst, family, style, weight, italic)
        opts = Options()
        opts.layout_features = ['*']
        opts.name_IDs = ['*']
        opts.name_languages = ['*']
        opts.notdef_outline = True
        opts.glyph_names = False
        sub = Subsetter(opts)
        sub.populate(unicodes=[cp for a, b in UNICODES for cp in range(a, b + 1)])
        sub.subset(inst)
        path = os.path.join(out_dir, out)
        inst.save(path)
        print(f'{out:34} {os.path.getsize(path):>7} bytes  {family} / {style} / {weight}')


if __name__ == '__main__':
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    build(sys.argv[1], sys.argv[2] if len(sys.argv) == 3 else os.path.dirname(os.path.abspath(__file__)))
