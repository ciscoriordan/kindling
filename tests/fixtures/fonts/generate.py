#!/usr/bin/env python3
"""Generate original, minimal static font fixtures (requires fonttools 4.63.0)."""
from pathlib import Path
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.pens.t2CharStringPen import T2CharStringPen

HERE = Path(__file__).resolve().parent


def generate(filename, family, weight=400, italic=False, typographic=None, cff=False):
    font = FontBuilder(1000, isTTF=not cff)
    font.setupGlyphOrder([".notdef", "space", "x"])
    font.setupCharacterMap({32: "space", 120: "x"})
    glyphs = {}
    for name in [".notdef", "space", "x"]:
        pen = T2CharStringPen(500, None) if cff else TTGlyphPen(None)
        if name != "space":
            pen.moveTo((100, 0))
            pen.lineTo((400, 0))
            pen.lineTo((400, 700))
            pen.lineTo((100, 700))
            pen.closePath()
        glyphs[name] = pen.getCharString() if cff else pen.glyph()
    if cff:
        font.setupCFF("KindlingCFF-Regular", {"FullName": family, "FamilyName": family, "Weight": "Regular"}, glyphs, {})
    else:
        font.setupGlyf(glyphs)
    font.setupHorizontalMetrics({name: (500, 100) for name in glyphs})
    font.setupHorizontalHeader(ascent=800, descent=-200)
    style = "Italic" if italic else "Bold" if weight == 700 else "Regular"
    names = {"familyName": family, "styleName": style, "uniqueFontIdentifier": f"{family}-{style}", "fullName": f"{family} {style}", "psName": f"{family.replace(' ', '')}-{style}", "version": "Version 1.0"}
    if typographic:
        names["typographicFamily"] = typographic
    font.setupNameTable(names)
    font.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200,
                 usWeightClass=weight, fsType=0, fsSelection=1 if italic else 32 if weight == 700 else 64)
    font.setupPost(italicAngle=-12 if italic else 0)
    font.setupMaxp()
    font.font["head"].created = font.font["head"].modified = 3800000000
    font.font["head"].macStyle = 2 if italic else 1 if weight == 700 else 0
    font.font.recalcTimestamp = False
    font.save(HERE / filename)


if __name__ == "__main__":
    generate("regular.ttf", "Kindling Test")
    generate("bold.ttf", "Kindling Test", 700)
    generate("italic.ttf", "Kindling Test", italic=True)
    generate("fallback.ttf", "Kindling Fallback", typographic="Kindling Preferred")
    generate("regular.otf", "Kindling CFF", cff=True)
