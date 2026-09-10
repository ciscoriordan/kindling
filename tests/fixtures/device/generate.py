#!/usr/bin/env python3
"""Generate the on-device test batch for the issues waiting on hardware.

The tracker's `needs-device-check` label means a fix is in the tree and nobody
has opened it on a Kindle yet. Those checks are expensive: the device is
ejected between rounds, so anything not copied in this mounted window waits for
the next one. This script builds every artifact for a whole round at once.

It deliberately does NOT reuse the repo's own fixtures. `clean_book` is a single
432-byte page, so "it opens but won't turn pages" looks like a bug and is just a
one-screen book; `parity/simple_comic` pages are flat color rectangles, which on
e-ink read as a rendering failure. Both wasted a device round in the past. The
fixtures here are built to be looked at: big numerals, borders, grey ramps,
several chapters of real text.

Every dictionary declares en -> en and the probe book is tagged `en`, because
the lookup popup's dictionary picker only lists dictionaries whose input
language matches the book's language tag. That is what makes one probe book able
to drive every dictionary in the round.

Usage:
    python3 generate.py [--kindling PATH] [--out DIR]
"""
import argparse
import os
import shutil
import subprocess
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
NBSP = " "

# The style block every dictionary carries, in the arrangement its issue needs.
# Both rules set a size nothing else on the screen has, so the popup answers the
# question from across the room. `u` sits before the escaped-colon rule and `i`
# sits after it, so the pair used to discriminate issue 39.
#
# Both rules now compile into inline markup at build time (issue 57), because
# the popup applies no stylesheet at all, so their position in the block should
# no longer change anything. That is the point of keeping four arrangements:
# all four must now look identical in the popup.
CONTROL_RULE = "u { font-size: 260%; font-weight: bold; }"
SIGNAL_RULE = "i { font-size: 260%; font-weight: bold; }"
TRAP_RULE = "idx\\:orth { display: block; }"

STYLE_PROBE_BODY = (
    "<p><u>UNDER</u> <i>ITAL</i> plain</p>"
    "<p>UNDER and ITAL must both be much larger than the word plain. "
    "If neither is, no dictionary styling reached the popup at all.</p>"
)


def esc(s):
    return (s.replace("&", "&amp;").replace("<", "&lt;")
             .replace(">", "&gt;").replace('"', "&quot;"))


def run(cmd, **kw):
    print("  $", " ".join(str(c) for c in cmd))
    r = subprocess.run(cmd, capture_output=True, text=True, **kw)
    if r.returncode != 0:
        print(r.stdout)
        print(r.stderr, file=sys.stderr)
        raise SystemExit(f"command failed: {' '.join(str(c) for c in cmd)}")
    return r


# --------------------------------------------------------------------------
# images
# --------------------------------------------------------------------------

FONT_CANDIDATES = [
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "/Library/Fonts/Arial.ttf",
]


def font(size):
    from PIL import ImageFont
    for path in FONT_CANDIDATES:
        if os.path.exists(path):
            try:
                return ImageFont.truetype(path, size)
            except OSError:
                continue
    return ImageFont.load_default()


def centered(draw, box, text, f, fill):
    l, t, r, b = draw.textbbox((0, 0), text, font=f)
    x = box[0] + (box[2] - box[0] - (r - l)) / 2 - l
    y = box[1] + (box[3] - box[1] - (b - t)) / 2 - t
    draw.text((x, y), text, font=f, fill=fill)


def comic_page(path, n, total, w, h, label):
    """A page that is unmistakable on e-ink: border, huge numeral, grey ramp."""
    from PIL import Image, ImageDraw
    img = Image.new("RGB", (w, h), (255, 255, 255))
    d = ImageDraw.Draw(img)
    m = max(4, w // 40)
    d.rectangle([m, m, w - m - 1, h - m - 1], outline=(0, 0, 0), width=max(3, w // 100))
    centered(d, (0, int(h * 0.10), w, int(h * 0.55)), str(n), font(int(h * 0.36)), (0, 0, 0))
    centered(d, (0, int(h * 0.56), w, int(h * 0.64)), f"page {n} of {total}", font(int(h * 0.045)), (0, 0, 0))
    centered(d, (0, int(h * 0.64), w, int(h * 0.72)), label, font(int(h * 0.045)), (0, 0, 0))
    # grey ramp: eleven steps, so posterization or a bad palette is obvious
    ramp_top, ramp_h = int(h * 0.76), int(h * 0.10)
    steps = 11
    for i in range(steps):
        v = int(255 * i / (steps - 1))
        x0 = m * 2 + (w - 4 * m) * i / steps
        x1 = m * 2 + (w - 4 * m) * (i + 1) / steps
        d.rectangle([x0, ramp_top, x1, ramp_top + ramp_h], fill=(v, v, v))
    d.rectangle([m * 2, ramp_top, w - m * 2, ramp_top + ramp_h], outline=(0, 0, 0), width=2)
    img.save(path, "PNG")


def alpha_page(path, n, total, w, h):
    """Opaque art on a fully transparent ground.

    The transparent ring must come back WHITE. Black means the alpha channel was
    dropped without compositing (issue #34).
    """
    from PIL import Image, ImageDraw
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    inset = int(min(w, h) * 0.18)
    d.rectangle([inset, inset, w - inset, h - inset], fill=(255, 255, 255, 255),
                outline=(0, 0, 0, 255), width=8)
    centered(d, (inset, inset, w - inset, int(h * 0.62)), str(n), font(int(h * 0.28)), (0, 0, 0, 255))
    centered(d, (inset, int(h * 0.62), w - inset, int(h * 0.74)), f"alpha {n}/{total}",
             font(int(h * 0.04)), (0, 0, 0, 255))
    centered(d, (0, int(h * 0.80), w, int(h * 0.90)),
             "margin should be WHITE", font(int(h * 0.035)), (0, 0, 0, 255))
    img.save(path, "PNG")


def solid_jpeg(path, w, h, rgb, label=None):
    """A plainly-coloured JPEG, so a photo of the library shelf answers which
    cover a tile was made from."""
    from PIL import Image, ImageDraw
    img = Image.new("RGB", (w, h), rgb)
    if label:
        d = ImageDraw.Draw(img)
        centered(d, (0, int(h * 0.45), w, int(h * 0.55)), label,
                 font(int(h * 0.06)), (255, 255, 255))
    img.save(path, "JPEG", quality=88)


def strip_app0_add_exif(path):
    """Rewrite a JFIF JPEG in place the way a camera or Photoshop export does:
    APP0 dropped, APP1/Exif first.

    kindling passes these straight through, so the cover record ships with no
    JFIF header at all, and therefore no density and no units (issue #43). That
    is the leading remaining explanation for #26's lock screen staying blank on
    devices nobody here can reproduce on, so it is worth an A/B rather than a
    guess in a comment.
    """
    import struct
    from PIL import Image
    b = open(path, "rb").read()
    assert b[:2] == b"\xff\xd8", "not a JPEG"
    i, segs = 2, []
    while i < len(b):
        if b[i] != 0xFF:
            break
        m = b[i + 1]
        if m in (0xD8, 0xD9) or 0xD0 <= m <= 0xD7:
            i += 2
            continue
        ln = struct.unpack(">H", b[i + 2:i + 4])[0]
        if m != 0xE0:
            segs.append(b[i:i + 2 + ln])
        i += 2 + ln
        if m == 0xDA:
            segs.append(b[i:])
            break
    ex = Image.Exif()
    ex[271] = "KindlingDeviceTest"
    ex[272] = "ExifFirstProbe"
    ex[274] = 1
    payload = b"Exif\x00\x00" + ex.tobytes()
    app1 = b"\xff\xe1" + struct.pack(">H", len(payload) + 2) + payload
    open(path, "wb").write(b"\xff\xd8" + app1 + b"".join(segs))


def alpha_cover(path, w=600, h=800):
    """A cover whose ground is fully transparent.

    Issue #34's remaining half: `build_thumbnail_record` drops the alpha channel
    without compositing, so this ships a black 330x440 library tile even though
    the comic path flattens correctly.
    """
    from PIL import Image, ImageDraw
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rectangle([120, 200, w - 120, h - 200], fill=(255, 255, 255, 255),
                outline=(0, 0, 0, 255), width=6)
    centered(d, (120, 200, w - 120, h - 260), "ALPHA", font(64), (0, 0, 0, 255))
    centered(d, (120, h - 300, w - 120, h - 220), "ground is transparent",
             font(26), (0, 0, 0, 255))
    img.save(path, "PNG")


def cover_image(path, title, sub, w=600, h=800):
    from PIL import Image, ImageDraw
    img = Image.new("RGB", (w, h), (18, 22, 26))
    d = ImageDraw.Draw(img)
    d.rectangle([24, 24, w - 25, h - 25], outline=(235, 235, 230), width=6)
    centered(d, (40, 180, w - 40, 380), title, font(72), (245, 245, 240))
    centered(d, (40, 400, w - 40, 520), sub, font(40), (170, 200, 215))
    centered(d, (40, h - 180, w - 40, h - 90), "kindling device test", font(30), (140, 150, 160))
    img.save(path, "JPEG", quality=90)


# --------------------------------------------------------------------------
# dictionary sources
# --------------------------------------------------------------------------

def entry(orth, body, visible=True, entry_id=None):
    """One <idx:entry>.

    `visible=False` emits a self-closing <idx:orth>, so the entry's rendered text
    is exactly `body` and nothing else. That is what makes byte-identical bodies
    across entries actually byte-identical, which is the condition issue #27
    reported and the reason the dedup workaround existed.
    """
    if visible:
        orth_tag = f'<idx:orth value="{esc(orth)}"><b>{esc(orth)}</b></idx:orth>'
    else:
        orth_tag = f'<idx:orth value="{esc(orth)}"/>'
    # reader.dict and PyGlossary put the entry's own anchor on the
    # <idx:entry> element, which is what a cross-reference aims at.
    id_attr = f' id="{esc(entry_id)}"' if entry_id else ""
    return (f'<idx:entry name="default" scriptable="yes"{id_attr}>{orth_tag}{body}'
            f"</idx:entry><mbp:pagebreak/>")


def dict_html(entries, style=None, head_attr="", link=None, title="Kindling device test"):
    head = ['<meta http-equiv="Content-Type" content="text/html; charset=utf-8"/>',
            f"<title>{esc(title)}</title>"]
    if link:
        head.append(f'<link rel="stylesheet" type="text/css" href="{link}"/>')
    if style:
        head.append(f'<style type="text/css">\n{style}\n</style>')
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE html>\n'
        '<html xmlns="http://www.w3.org/1999/xhtml" '
        'xmlns:idx="http://www.mobipocket.com/idx" '
        'xmlns:mbp="http://www.mobipocket.com" xml:lang="en" lang="en">\n'
        f"<head{head_attr}>" + "".join(head) + "</head>\n"
        "<body><mbp:frameset>\n" + "\n".join(entries) + "\n</mbp:frameset></body></html>\n"
    )


def dict_opf(title, files, css=None, uid="kindling-device"):
    manifest = ['<item id="cover-img" href="cover.jpg" media-type="image/jpeg" properties="coverimage"/>',
                '<item id="usage" href="usage.html" media-type="application/xhtml+xml"/>',
                '<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>']
    spine = ['<itemref idref="usage"/>']
    for i, f in enumerate(files):
        manifest.append(f'<item id="c{i}" href="{f}" media-type="application/xhtml+xml"/>')
        spine.append(f'<itemref idref="c{i}"/>')
    if css:
        manifest.append(f'<item id="css" href="{css}" media-type="text/css"/>')
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<package version="2.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId">\n'
        '  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">\n'
        f"    <dc:title>{esc(title)}</dc:title>\n"
        "    <dc:language>en</dc:language>\n"
        "    <dc:creator>Kindling device test</dc:creator>\n"
        f'    <dc:identifier id="BookId">{esc(uid)}</dc:identifier>\n'
        '    <meta name="cover" content="cover-img"/>\n'
        "    <x-metadata>\n"
        "      <DictionaryInLanguage>en</DictionaryInLanguage>\n"
        "      <DictionaryOutLanguage>en</DictionaryOutLanguage>\n"
        "      <DefaultLookupIndex>default</DefaultLookupIndex>\n"
        "    </x-metadata>\n"
        "  </metadata>\n"
        "  <manifest>\n    " + "\n    ".join(manifest) + "\n  </manifest>\n"
        '  <spine toc="ncx">\n    ' + "\n    ".join(spine) + "\n  </spine>\n"
        "  <guide>\n"
        f'    <reference type="index" title="Dictionary" href="{files[0]}"/>\n'
        "  </guide>\n"
        "</package>\n"
    )


def write_common(d, title, blurb, uid):
    cover_image(os.path.join(d, "cover.jpg"), title.split()[0], title.split(None, 1)[-1])
    open(os.path.join(d, "usage.html"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE html>\n'
        '<html xmlns="http://www.w3.org/1999/xhtml"><head>'
        '<meta http-equiv="Content-Type" content="text/html; charset=utf-8"/>'
        f"<title>About</title></head><body><h1>{esc(title)}</h1><p>{esc(blurb)}</p>"
        "</body></html>\n")
    open(os.path.join(d, "toc.ncx"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1" xml:lang="en">\n'
        f'<head><meta name="dtb:uid" content="{esc(uid)}"/>'
        '<meta name="dtb:depth" content="1"/><meta name="dtb:totalPageCount" content="0"/>'
        '<meta name="dtb:maxPageNumber" content="0"/></head>\n'
        f"<docTitle><text>{esc(title)}</text></docTitle>\n"
        '<navMap><navPoint id="np1" playOrder="1"><navLabel><text>About</text></navLabel>'
        '<content src="usage.html"/></navPoint></navMap>\n</ncx>\n')


# The words the probe book prints, grouped by what they prove.
# Letters only, deliberately. A Kindle will not select a word across a
# letter-to-digit boundary, so tapping "zdup01" in the probe book selects
# "zdup" and the lookup misses. That cost a whole device round in the
# 2026-09-09 session, where thirty of these and ten of the ones below read
# as a broken index and were nothing of the sort: typing "zdup01" into the
# search box resolved it correctly every time.
_DUP_SUFFIXES = [a + b for a in "abcde" for b in "abcdef"]
DUP_WORDS = [f"zdup{sfx}" for sfx in _DUP_SUFFIXES[:30]]
NBSP_HEAD = f"znbspalpha{NBSP}znbspbeta"
NOLIMIT_WORDS = [f"znolimit{sfx}" for sfx in
                 ["alpha", "beta", "gamma", "delta", "epsilon",
                  "zeta", "eta", "theta", "iota", "kappa"]]
FILLER = [("zapple", "A common fruit, here only so the dictionary is not all test words."),
          ("zbridge", "A structure carrying a road over an obstacle."),
          ("zcandle", "A cylinder of wax with a wick."),
          ("zdelta", "The fourth letter of the Greek alphabet."),
          ("zember", "A small piece of burning coal or wood.")]

SHARED_BODY = "<p>Shared body. Every zdup entry ships these exact bytes and no headword of its own.</p>"


def style_entry():
    return entry("zstyle", STYLE_PROBE_BODY)


def filler_entries():
    return [entry(w, f"<p>{esc(g)}</p>") for w, g in FILLER]


def build_dict_a(root):
    """#27 duplicate bodies, #36 nbsp headword, #39 escaped-colon reorder."""
    d = os.path.join(root, "src", "dict-a")
    os.makedirs(d, exist_ok=True)
    style = f"p {{ margin: 0.3em 0; }}\n{CONTROL_RULE}\n{TRAP_RULE}\n{SIGNAL_RULE}\n"
    entries = [style_entry()]
    entries += [entry(w, SHARED_BODY, visible=False) for w in DUP_WORDS]
    entries.append(entry(NBSP_HEAD,
                         "<p>Resolved through the plain-space alias. The stored headword "
                         "carries U+00A0 between the two words.</p>"))
    entries += filler_entries()
    open(os.path.join(d, "content.html"), "w", encoding="utf-8").write(
        dict_html(entries, style=style, title="KD-A inline style"))
    open(os.path.join(d, "dict-a.opf"), "w", encoding="utf-8").write(
        dict_opf("KD-A inline style", ["content.html"], uid="kindling-device-a"))
    write_common(d, "KD-A inline style",
                 "Escaped-colon rule sits in the middle of the style block (issue #39), "
                 "thirty byte-identical bodies (#27), and a non-breaking-space headword (#36).", uid="kindling-device-a")
    return d, "dict-a.opf"


def build_dict_b(root):
    """#40 case 1: an attribute on <head> hid the style block."""
    d = os.path.join(root, "src", "dict-b")
    os.makedirs(d, exist_ok=True)
    style = f"p {{ margin: 0.3em 0; }}\n{CONTROL_RULE}\n{SIGNAL_RULE}\n{TRAP_RULE}\n"
    entries = [style_entry()] + filler_entries()
    open(os.path.join(d, "content.html"), "w", encoding="utf-8").write(
        dict_html(entries, style=style, head_attr=' profile="http://www.w3.org/2005/10/profile"',
                  title="KD-B head profile"))
    open(os.path.join(d, "dict-b.opf"), "w", encoding="utf-8").write(
        dict_opf("KD-B head profile", ["content.html"], uid="kindling-device-b"))
    write_common(d, "KD-B head profile",
                 "The style block lives under <head profile=\"...\">, which used to hide it "
                 "from the head regex entirely (issue #40).", uid="kindling-device-b")
    return d, "dict-b.opf"


def build_dict_c(root):
    """#40 case 2: an external stylesheet never reached the dictionary path."""
    d = os.path.join(root, "src", "dict-c")
    os.makedirs(d, exist_ok=True)
    open(os.path.join(d, "dict.css"), "w", encoding="utf-8").write(
        f"p {{ margin: 0.3em 0; }}\n{CONTROL_RULE}\n{SIGNAL_RULE}\n")
    entries = [style_entry()] + filler_entries()
    open(os.path.join(d, "content.html"), "w", encoding="utf-8").write(
        dict_html(entries, link="dict.css", title="KD-C external css"))
    open(os.path.join(d, "dict-c.opf"), "w", encoding="utf-8").write(
        dict_opf("KD-C external css", ["content.html"], css="dict.css", uid="kindling-device-c"))
    write_common(d, "KD-C external css",
                 "All styling comes from a linked .css file, which the dictionary path used to "
                 "ignore completely (issue #40).", uid="kindling-device-c")
    return d, "dict-c.opf"


def build_dict_d(root):
    """#40 case 3: only the first file's <style> survived.

    Both files carry a style block and they carry different halves of the probe,
    which is what makes this discriminate. Under the old assembler only file
    one's sheet reached the text blob, so UNDER came out large and ITAL did not.
    An earlier draft of this fixture put no style in file one at all, and the
    pre-fix binary passed it: the assembler kept the first sheet it FOUND, and
    with file one empty that was file two's.
    """
    d = os.path.join(root, "src", "dict-d")
    os.makedirs(d, exist_ok=True)
    open(os.path.join(d, "one.html"), "w", encoding="utf-8").write(
        dict_html(filler_entries(), style=f"p {{ margin: 0.3em 0; }}\n{CONTROL_RULE}\n",
                  title="KD-D second file"))
    open(os.path.join(d, "two.html"), "w", encoding="utf-8").write(
        dict_html([style_entry()], style=SIGNAL_RULE + "\n", title="KD-D second file"))
    open(os.path.join(d, "dict-d.opf"), "w", encoding="utf-8").write(
        dict_opf("KD-D second file", ["one.html", "two.html"], uid="kindling-device-d"))
    write_common(d, "KD-D second file",
                 "File one styles u, file two styles i. The assembler used to keep the first "
                 "file's sheet and drop every later one (issue #40).", uid="kindling-device-d")
    return d, "dict-d.opf"


def build_dict_e(root):
    """#41: --no-kindle-limits used to switch the assembler and lose the anchors."""
    d = os.path.join(root, "src", "dict-e")
    os.makedirs(d, exist_ok=True)
    entries = [style_entry()]
    for i, w in enumerate(NOLIMIT_WORDS, 1):
        # Body deliberately does NOT open with the headword in <b>/<big>, which
        # is the shape the old markup-search fallback could not anchor.
        entries.append(entry(w, f"<p>Entry number {i} resolved with its body intact. "
                                f"A blank popup here means the anchors were lost.</p>",
                             visible=False))
    entries += filler_entries()
    style = f"p {{ margin: 0.3em 0; }}\n{CONTROL_RULE}\n{SIGNAL_RULE}\n"
    open(os.path.join(d, "content.html"), "w", encoding="utf-8").write(
        dict_html(entries, style=style, title="KD-E no kindle limits"))
    open(os.path.join(d, "dict-e.opf"), "w", encoding="utf-8").write(
        dict_opf("KD-E no kindle limits", ["content.html"], uid="kindling-device-e"))
    write_common(d, "KD-E no kindle limits",
                 "Built with --no-kindle-limits. Bodies do not repeat their headword in bold, "
                 "the shape that popped up blank under the old fallback (issue #41).", uid="kindling-device-e")
    return d, "dict-e.opf"


def build_dict_g(root):
    """#56 list markers: what a MOBI7 popup will actually draw.

    reader.dict numbers senses with <ol> and letters or roman numerals its
    sub-senses with list-style-type. kindling flattens every level to decimal
    by writing value="N" on each <li>, because that is the one marker the
    popup was known to draw (issue #16, device-verified). Whether anything
    else works has never been tested, and two cheap possibilities have to be
    ruled in or out before the code changes:

      LIST-TYPE   <ol type="a"> with value="1" on each item. kindling passes
                  `type` through today and kindlegen's own output proves the
                  combination is legal MOBI7. If the renderer takes the glyph
                  from `type` and the ordinal from `value`, nothing else is
                  needed and the fix is one attribute.
      LIST-TEXT   the marker written into the item text. This is what a
                  compile-to-literal-markup fix would produce, and because
                  kindling also writes value="1" on the same item, this entry
                  shows whether the two stack into "1. a." — which is the
                  failure mode that would make that fix unusable.
      LIST-PLAIN  an ordinary decimal <ol>, the control. If this one draws no
                  numbers either, the popup is ignoring list markup entirely
                  and neither approach can work.

    0.22.1 shipped <ol type="a"> on its own, with no value, and the device
    drew nothing at all; that is why the decimal flattening exists. This probe
    differs in pairing `type` with `value`, which is the combination that was
    never tried.
    """
    d = os.path.join(root, "src", "dict-g")
    os.makedirs(d, exist_ok=True)
    body = (
        "<p>LIST-TYPE, expect a. b. c.</p>"
        '<ol type="a"><li>ALPHA item one</li><li>ALPHA item two</li>'
        "<li>ALPHA item three</li></ol>"
        "<p>LIST-TEXT, expect a. b. and NOT 1. a.</p>"
        "<ol><li>a. TEXT item one</li><li>b. TEXT item two</li></ol>"
        "<p>LIST-PLAIN control, expect 1. 2.</p>"
        "<ol><li>PLAIN item one</li><li>PLAIN item two</li></ol>"
        "<p>LIST-BULLET control, expect bullets.</p>"
        "<ul><li>BULLET item one</li><li>BULLET item two</li></ul>"
    )
    entries = [entry("zlistprobe", body)]
    entries += filler_entries()
    open(os.path.join(d, "content.html"), "w", encoding="utf-8").write(
        dict_html(entries, title="KD-G list markers"))
    open(os.path.join(d, "dict-g.opf"), "w", encoding="utf-8").write(
        dict_opf("KD-G list markers", ["content.html"], uid="kindling-device-g"))
    write_common(d, "KD-G list markers",
                 "Look up zlistprobe and read the four lists. Which markers appear "
                 "decides whether lettered sub-senses can be rendered at all (issue 56).",
                 uid="kindling-device-g")
    return d, "dict-g.opf"


def build_dict_h(root):
    """#54 cross-references: an entry that says "cf. zeta" with a link on it.

    Two files, because resolution is per source file and that is the half a
    single-file fixture cannot test. Each file defines its own <a id="dup">,
    and each has a bare #dup link: a resolver with one table across the merged
    book sends both to whichever it saw first, which is a live link to the
    WRONG definition and looks completely normal on screen. The two DUPTARGET
    lines are labelled so a photo says which one was reached.

    Note before testing: a Kindle lookup popup disables links, the same thing
    that made the footnote arrows in #50 look dead. These have to be read as
    book pages, through Go To, not tapped in a popup.
    """
    d = os.path.join(root, "src", "dict-h")
    os.makedirs(d, exist_ok=True)
    one = [
        entry("zalpha",
              "<p>First letter. Cross-file link: <a href=\"content_02.html#hw_zzeta\">"
              "TO ZZETA</a>. Same-file link: <a href=\"#hw_zbeta\">TO ZBETA</a>.</p>"
              "<p><a id=\"dup\">DUPTARGET IN FILE ONE</a></p>", entry_id="hw_zalpha"),
        entry("zbeta",
              "<p>Second letter. This is where TO ZBETA must land.</p>"
              "<p>Bare fragment: <a href=\"#dup\">TO DUP, FILE ONE</a> must reach "
              "DUPTARGET IN FILE ONE, not the one in file two.</p>", entry_id="hw_zbeta"),
    ]
    two = [
        entry("zzeta",
              "<p>Sixth letter. This is where TO ZZETA must land. Back: "
              "<a href=\"content_01.html#hw_zalpha\">TO ZALPHA</a>.</p>"
              "<p><a id=\"dup\">DUPTARGET IN FILE TWO</a></p>", entry_id="hw_zzeta"),
        entry("zomega",
              "<p>Last letter. Bare fragment: <a href=\"#dup\">TO DUP, FILE TWO</a> "
              "must reach DUPTARGET IN FILE TWO.</p>"
              "<p>Dead link, must do nothing at all: "
              "<a href=\"#hw_znosuchword\">TO NOWHERE</a>.</p>", entry_id="hw_zomega"),
    ]
    open(os.path.join(d, "content_01.html"), "w", encoding="utf-8").write(
        dict_html(one, title="KD-H cross references"))
    open(os.path.join(d, "content_02.html"), "w", encoding="utf-8").write(
        dict_html(two, title="KD-H cross references"))
    open(os.path.join(d, "dict-h.opf"), "w", encoding="utf-8").write(
        dict_opf("KD-H cross references", ["content_01.html", "content_02.html"],
                 uid="kindling-device-h"))
    write_common(d, "KD-H cross references",
                 "Read as a book, not through the popup: a Kindle popup disables links. "
                 "Each labelled link must reach the line its text names (issue 54).",
                 uid="kindling-device-h")
    return d, "dict-h.opf"


def build_dict_i(root):
    """#42 content outside <idx:entry>: everything that used to be dropped.

    A letter heading before the first entry, a note between two entries, an
    entry the parser rejects because it has no <idx:orth>, a second heading,
    and a closing paragraph after the last entry. All of it used to vanish
    with exit 0.

    The other half of the check is that the three real entries still index to
    their own headwords rather than to the heading in front of them. A run
    that steals an entry's anchor is issue #27 with a new cause, and it looks
    like a working dictionary until you tap a word.
    """
    d = os.path.join(root, "src", "dict-i")
    os.makedirs(d, exist_ok=True)
    parts = [
        "<h2>HEADING BEFORE FIRST ENTRY</h2>",
        entry("zgapapple", "<p>Definition of zgapapple. Its popup must show THIS "
                           "line and not the heading above it.</p>"),
        "<p>NOTE BETWEEN TWO ENTRIES</p>",
        entry("zgapbanana", "<p>Definition of zgapbanana. Its popup must show THIS "
                            "line and not the note above it.</p>"),
        '<idx:entry name="default" scriptable="yes">'
        "<p>REJECTED ENTRY WITH NO HEADWORD, must render as ordinary prose</p>"
        "</idx:entry>",
        "<h2>HEADING BEFORE LAST ENTRY</h2>",
        entry("zgapcherry", "<p>Definition of zgapcherry. Its popup must show THIS "
                            "line and not the heading above it.</p>"),
        "<p>CLOSING PARAGRAPH AFTER LAST ENTRY</p>",
    ]
    open(os.path.join(d, "content.html"), "w", encoding="utf-8").write(
        dict_html(parts, title="KD-I gap content"))
    open(os.path.join(d, "dict-i.opf"), "w", encoding="utf-8").write(
        dict_opf("KD-I gap content", ["content.html"], uid="kindling-device-i"))
    write_common(d, "KD-I gap content",
                 "Read as a book: all five capitalised lines must be visible. Then look "
                 "up each zgap word and check its popup shows its own definition rather "
                 "than the heading in front of it (issue 42).",
                 uid="kindling-device-i")
    return d, "dict-i.opf"


def build_dict_f(root):
    """#53 popup scroll-through: the <hr/> -> <hr/><mbp:pagebreak/> fix.

    The first entry is long enough to force scrolling, and the neighbor's
    bold headword is the first thing after the separator — exactly the reach
    a bare <hr/> boundary gives the popup, so the check fails pre-fix.
    """
    d = os.path.join(root, "src", "dict-f")
    os.makedirs(d, exist_ok=True)
    paras = [
        "<p>Look up <b>zbleedfirst</b> in the probe book, then scroll this "
        "definition to its very bottom.</p>",
    ]
    for i in range(1, 7):
        paras.append(f"<p>Filler paragraph {i} of zbleedfirst. It is here only to push "
                     f"this entry past one popup screen, because the scroll-through only "
                     f"shows while scrolling a definition that does not fit.</p>")
    paras.append("<p>LAST LINE OF zbleedfirst. Any word, rule or paragraph readable "
                 "below this line is the next entry bleeding into the popup.</p>")
    entries = [
        entry("zbleedfirst", "".join(paras)),
        entry("zbleedsecond", "<p>zbleedsecond is a different entry. Seeing this text, or "
                              "this bold headword, while looking up zbleedfirst is the "
                              "failure.</p>"),
    ]
    entries += filler_entries()
    open(os.path.join(d, "content.html"), "w", encoding="utf-8").write(
        dict_html(entries, title="KD-F popup boundary"))
    open(os.path.join(d, "dict-f.opf"), "w", encoding="utf-8").write(
        dict_opf("KD-F popup boundary", ["content.html"], uid="kindling-device-f"))
    write_common(d, "KD-F popup boundary",
                 "Look up zbleedfirst and scroll to the bottom. The LAST LINE paragraph "
                 "must be the end of the popup; zbleedsecond appearing below it is the "
                 "scroll-through bug (bare <hr/> entry separators).", uid="kindling-device-f")
    return d, "dict-f.opf"


# --------------------------------------------------------------------------
# books
# --------------------------------------------------------------------------

CHAPTER_TEXT = [
    "The record table is the first thing a reader touches and the last thing an "
    "author thinks about. Every offset in it is absolute, every one of them is "
    "big-endian, and a single byte out of place takes the whole file with it.",
    "Compression came later than the format did, which is why the header carries a "
    "field for it at all. A file can declare no compression and still be perfectly "
    "legal; it will simply be four times the size it needed to be.",
    "Trailing bytes are the part nobody documents. They hang off the end of each "
    "text record, they are counted from the back, and their presence is announced by "
    "two bits in a field that predates them by a decade.",
]


def book_source(root, name, title, uid, cover_prop, cover_name="cover.jpg", exif_first=False):
    """A short book with a cover and three real chapters.

    `cover_prop` picks which manifest spelling declares the cover. The EPUB 3
    spelling is `cover-image`; `coverimage` is the non-standard one kindling used
    to be the only one to match (issue #30).
    """
    d = os.path.join(root, "src", name)
    os.makedirs(d, exist_ok=True)
    if cover_name.endswith(".png"):
        alpha_cover(os.path.join(d, cover_name))
    else:
        cover_image(os.path.join(d, cover_name), title.split()[0], title.split(None, 1)[-1])
        if exif_first:
            strip_app0_add_exif(os.path.join(d, cover_name))
    for i, para in enumerate(CHAPTER_TEXT, 1):
        open(os.path.join(d, f"ch{i}.html"), "w", encoding="utf-8").write(
            '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE html>\n'
            '<html xmlns="http://www.w3.org/1999/xhtml"><head>'
            '<meta http-equiv="Content-Type" content="text/html; charset=utf-8"/>'
            f"<title>Chapter {i}</title></head><body>"
            f"<h1>Chapter {i}</h1><p>{esc(para)}</p><p>{esc(para)}</p>"
            "</body></html>\n")
    mime = "image/png" if cover_name.endswith(".png") else "image/jpeg"
    manifest = [f'<item id="cover-img" href="{cover_name}" media-type="{mime}" properties="{cover_prop}"/>']
    spine = []
    for i in range(1, len(CHAPTER_TEXT) + 1):
        manifest.append(f'<item id="ch{i}" href="ch{i}.html" media-type="application/xhtml+xml"/>')
        spine.append(f'<itemref idref="ch{i}"/>')
    manifest.append('<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>')
    # No <meta name="cover"> on purpose: properties= has to carry it alone.
    open(os.path.join(d, f"{name}.opf"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<package version="3.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId">\n'
        '  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">\n'
        f"    <dc:title>{esc(title)}</dc:title>\n"
        "    <dc:language>en</dc:language>\n"
        "    <dc:creator>Kindling device test</dc:creator>\n"
        f'    <dc:identifier id="BookId">{esc(uid)}</dc:identifier>\n'
        "  </metadata>\n"
        "  <manifest>\n    " + "\n    ".join(manifest) + "\n  </manifest>\n"
        '  <spine toc="ncx">\n    ' + "\n    ".join(spine) + "\n  </spine>\n"
        "</package>\n")
    open(os.path.join(d, "toc.ncx"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1" xml:lang="en">\n'
        f'<head><meta name="dtb:uid" content="{esc(uid)}"/>'
        '<meta name="dtb:depth" content="1"/><meta name="dtb:totalPageCount" content="0"/>'
        '<meta name="dtb:maxPageNumber" content="0"/></head>\n'
        f"<docTitle><text>{esc(title)}</text></docTitle>\n"
        '<navMap>' + "".join(
            f'<navPoint id="np{i}" playOrder="{i}"><navLabel><text>Chapter {i}</text></navLabel>'
            f'<content src="ch{i}.html"/></navPoint>' for i in range(1, len(CHAPTER_TEXT) + 1)
        ) + "</navMap>\n</ncx>\n")
    return d, f"{name}.opf"


def footnote_book(root):
    """A book whose footnotes have to land on the right note (issue #50).

    Every chapter names its own marker `ref-1` and its own tail `end-N`, which
    is what real books do and what makes this worth checking: three back-links
    all ask for `ref-1`, and resolving that name in one table across the whole
    book still produces links that go somewhere, just always to chapter one.
    So each note names the chapter it belongs to in text large enough to read
    from a photo, and landing on the wrong one is visible rather than subtle.

    Both directions are on the page. The marker goes to the note, the note's
    arrow comes back to the marker, and a separate link inside each chapter
    jumps to the bottom of that same chapter without leaving it.
    """
    name = "book-footnotes"
    d = os.path.join(root, "src", name)
    os.makedirs(d, exist_ok=True)
    cover_image(os.path.join(d, "cover.jpg"), "KF", "footnote links")
    uid = "kindling-device-footnotes"
    big = 'style="font-size: 200%; font-weight: bold;"'

    files = []
    for i, para in enumerate(CHAPTER_TEXT, 1):
        files.append(f"ch{i}.html")
        open(os.path.join(d, f"ch{i}.html"), "w", encoding="utf-8").write(
            '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE html>\n'
            '<html xmlns="http://www.w3.org/1999/xhtml" '
            'xmlns:epub="http://www.idpf.org/2007/ops"><head>'
            '<meta http-equiv="Content-Type" content="text/html; charset=utf-8"/>'
            f"<title>Chapter {i}</title></head><body>"
            f'<h1 {big}>CHAPTER {i}</h1>'
            f'<p>{esc(para)}<a href="notes.html#ftn-{i}" id="ref-1" '
            f'epub:type="noteref" role="doc-noteref">[1]</a></p>'
            f'<p>Tapping that marker must open a note that says NOTE FOR CHAPTER {i}. '
            f'Any other number means every chapter\'s footnote went to the same place.</p>'
            f'<p><a href="#end-{i}">Jump to the end of this chapter.</a> '
            f"It must stay in chapter {i}.</p>"
            f"<p>{esc(para)}</p><p>{esc(para)}</p>"
            f'<p id="end-{i}" {big}>END OF CHAPTER {i}</p>'
            "</body></html>\n")

    body = "".join(
        f'<p id="ftn-{i}" epub:type="footnote" role="doc-footnote">'
        f'<a href="ch{i}.html#ref-1" {big}>&#8592;</a> '
        f'<span {big}>NOTE FOR CHAPTER {i}</span>. '
        f"The arrow must return to the marker in chapter {i}.</p>"
        for i in range(1, len(CHAPTER_TEXT) + 1))
    files.append("notes.html")
    open(os.path.join(d, "notes.html"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE html>\n'
        '<html xmlns="http://www.w3.org/1999/xhtml" '
        'xmlns:epub="http://www.idpf.org/2007/ops"><head>'
        '<meta http-equiv="Content-Type" content="text/html; charset=utf-8"/>'
        "<title>Notes</title></head><body>"
        f'<h1 {big}>NOTES</h1>{body}</body></html>\n')

    manifest = ['<item id="cover-img" href="cover.jpg" media-type="image/jpeg" '
                'properties="cover-image"/>']
    spine = []
    for n, f in enumerate(files):
        manifest.append(f'<item id="s{n}" href="{f}" media-type="application/xhtml+xml"/>')
        spine.append(f'<itemref idref="s{n}"/>')
    manifest.append('<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>')
    open(os.path.join(d, f"{name}.opf"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<package version="3.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId">\n'
        '  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">\n'
        "    <dc:title>KF footnote links</dc:title>\n"
        "    <dc:language>en</dc:language>\n"
        "    <dc:creator>Kindling device test</dc:creator>\n"
        f'    <dc:identifier id="BookId">{esc(uid)}</dc:identifier>\n'
        "  </metadata>\n"
        "  <manifest>\n    " + "\n    ".join(manifest) + "\n  </manifest>\n"
        '  <spine toc="ncx">\n    ' + "\n    ".join(spine) + "\n  </spine>\n"
        "</package>\n")
    open(os.path.join(d, "toc.ncx"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1" xml:lang="en">\n'
        f'<head><meta name="dtb:uid" content="{esc(uid)}"/>'
        '<meta name="dtb:depth" content="1"/><meta name="dtb:totalPageCount" content="0"/>'
        '<meta name="dtb:maxPageNumber" content="0"/></head>\n'
        "<docTitle><text>KF footnote links</text></docTitle>\n"
        '<navMap>' + "".join(
            f'<navPoint id="np{n}" playOrder="{n + 1}"><navLabel><text>'
            f'{"Notes" if f == "notes.html" else f"Chapter {n + 1}"}</text></navLabel>'
            f'<content src="{f}"/></navPoint>' for n, f in enumerate(files)
        ) + "</navMap>\n</ncx>\n")
    return d, f"{name}.opf"


def probe_book(root):
    """The book whose words get tapped. Tagged `en` so every test dict lists."""
    d = os.path.join(root, "src", "probe")
    os.makedirs(d, exist_ok=True)
    cover_image(os.path.join(d, "cover.jpg"), "KP", "probe book")

    def section(num, heading, lead, words, cols=5):
        rows = []
        for i in range(0, len(words), cols):
            rows.append("<tr>" + "".join(f"<td>{esc(w)}</td>" for w in words[i:i + cols]) + "</tr>")
        table = "<table>" + "".join(rows) + "</table>" if words else ""
        return (f"<h2>{esc(heading)}</h2><p>{esc(lead)}</p>{table}")

    body = [
        "<h1>Kindling device probe</h1>",
        "<p>Tap a word below, then use the dictionary name at the bottom of the popup "
        "to switch dictionaries. Every test dictionary here is English to English, so "
        "all six appear in that list.</p>",
        section(1, "1. Style, in all four dictionaries (issue 57)",
                "Look up zstyle, then run it through KD-A, KD-B, KD-C and KD-D in turn. "
                "In each one, UNDER and ITAL must both be much larger than the word plain, "
                "UNDER underlined and ITAL italic. The four dictionaries carry the same two "
                "rules in four different arrangements, and the rules are now compiled into "
                "the entry markup at build time rather than left to the popup, so all four "
                "must look the same. Any dictionary that shows plain text is one whose "
                "stylesheet never reached the entries.",
                ["zstyle"], cols=1),
        section(2, "2. Identical bodies, in KD-A (issue 27)",
                "Every one of these has the same body bytes and no headword of its own. "
                "Tap several, spread out across the list. Each must open a popup whose "
                "headword matches the word you tapped.",
                DUP_WORDS),
        section(3, "3. Non-breaking space, in KD-A (issue 36)",
                "Select both words together. The stored headword has a non-breaking space "
                "between them, so this only resolves through the plain-space alias.",
                ["znbspalpha znbspbeta"], cols=1),
        section(4, "4. No kindle limits, in KD-E (issue 41)",
                "Switch to KD-E first. Each of these must show its numbered body text. "
                "A popup that opens but is blank is the bug.",
                NOLIMIT_WORDS),
        section(5, "5. Popup boundary, in KD-F (issue 53)",
                "Switch to KD-F. Look up zbleedfirst and scroll the popup to its very "
                "bottom. The LAST LINE paragraph must be the end of the popup. Seeing "
                "zbleedsecond, its bold headword, or anything else below that line is "
                "the bug.",
                ["zbleedfirst"], cols=1),
        section(6, "6. List markers, in KD-G (issue 56)",
                "Switch to KD-G. Look up zlistprobe. Four lists follow each other. "
                "Report exactly what marker each item shows, including nothing at all. "
                "LIST-TYPE should read a. b. c.; LIST-TEXT should read a. b. and NOT "
                "1. a.; LIST-PLAIN should read 1. 2.; LIST-BULLET should show bullets. "
                "If LIST-PLAIN has no numbers either, the popup is ignoring list "
                "markup entirely and the other three answers do not matter.",
                ["zlistprobe"], cols=1),
        section(7, "7. Cross-references, in KD-H (issue 54)",
                "Do NOT use the popup for this one: a Kindle popup disables links. "
                "Open KD-H as a book and go to its entries. Every capitalised link "
                "must reach the line its own text names. The two TO DUP links matter "
                "most: TO DUP, FILE ONE must reach DUPTARGET IN FILE ONE and TO DUP, "
                "FILE TWO must reach DUPTARGET IN FILE TWO. Reaching the other file's "
                "DUPTARGET is a live link to the wrong definition. TO NOWHERE must do "
                "nothing at all.",
                ["zalpha"], cols=1),
        section(8, "8. Text between entries, in KD-I (issue 42)",
                "Open KD-I as a book: HEADING BEFORE FIRST ENTRY, NOTE BETWEEN TWO "
                "ENTRIES, REJECTED ENTRY WITH NO HEADWORD, HEADING BEFORE LAST ENTRY "
                "and CLOSING PARAGRAPH AFTER LAST ENTRY must all be visible; every one "
                "of them used to be dropped silently. Then look up the three words "
                "below and check each popup shows its own definition rather than the "
                "heading in front of it.",
                ["zgapapple", "zgapbanana", "zgapcherry"]),
        "<h2>9. Controls</h2><p>These are ordinary entries in every dictionary. If they "
        "fail too, something is wrong with the round rather than with the fix.</p>"
        "<table><tr>" + "".join(f"<td>{w}</td>" for w, _ in FILLER) + "</tr></table>",
    ]
    open(os.path.join(d, "probe.html"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE html>\n'
        '<html xmlns="http://www.w3.org/1999/xhtml"><head>'
        '<meta http-equiv="Content-Type" content="text/html; charset=utf-8"/>'
        "<title>Kindling device probe</title>"
        "<style type=\"text/css\">td { padding: 0.35em 0.7em; } "
        "h2 { margin-top: 1.4em; } table { margin: 0.6em 0; }</style>"
        "</head><body>" + "".join(body) + "</body></html>\n")
    open(os.path.join(d, "probe.opf"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<package version="2.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId">\n'
        '  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">\n'
        "    <dc:title>KP probe book</dc:title>\n"
        "    <dc:language>en</dc:language>\n"
        "    <dc:creator>Kindling device test</dc:creator>\n"
        '    <dc:identifier id="BookId">kindling-device-probe</dc:identifier>\n'
        '    <meta name="cover" content="cover-img"/>\n'
        "  </metadata>\n"
        "  <manifest>\n"
        '    <item id="cover-img" href="cover.jpg" media-type="image/jpeg" properties="coverimage"/>\n'
        '    <item id="probe" href="probe.html" media-type="application/xhtml+xml"/>\n'
        '    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>\n'
        "  </manifest>\n"
        '  <spine toc="ncx">\n    <itemref idref="probe"/>\n  </spine>\n'
        "</package>\n")
    open(os.path.join(d, "toc.ncx"), "w", encoding="utf-8").write(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1" xml:lang="en">\n'
        '<head><meta name="dtb:uid" content="kindling-device-probe"/>'
        '<meta name="dtb:depth" content="1"/><meta name="dtb:totalPageCount" content="0"/>'
        '<meta name="dtb:maxPageNumber" content="0"/></head>\n'
        "<docTitle><text>KP probe book</text></docTitle>\n"
        '<navMap><navPoint id="np1" playOrder="1"><navLabel><text>Probe</text></navLabel>'
        '<content src="probe.html"/></navPoint></navMap>\n</ncx>\n')
    return d, "probe.opf"


# --------------------------------------------------------------------------
# comics
# --------------------------------------------------------------------------

def make_cbz(root, name, pages_fn, count, w, h, label=None):
    d = os.path.join(root, "src", name)
    os.makedirs(d, exist_ok=True)
    names = []
    for n in range(1, count + 1):
        p = os.path.join(d, f"{n:03d}.png")
        pages_fn(p, n, count, w, h, label) if label is not None else pages_fn(p, n, count, w, h)
        names.append(p)
    cbz = os.path.join(root, "src", f"{name}.cbz")
    with zipfile.ZipFile(cbz, "w", zipfile.ZIP_DEFLATED) as z:
        for p in names:
            z.write(p, os.path.basename(p))
    return cbz


# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kindling", default=os.environ.get("KINDLING", "kindling-cli"))
    ap.add_argument("--out", default=os.path.join(HERE, "build"))
    args = ap.parse_args()
    K = args.kindling
    out = os.path.abspath(args.out)
    if os.path.isdir(out):
        shutil.rmtree(out)
    os.makedirs(os.path.join(out, "src"), exist_ok=True)
    ship = os.path.join(out, "ship")
    os.makedirs(ship, exist_ok=True)

    print("dictionaries")
    for builder, flags in ((build_dict_a, []), (build_dict_b, []), (build_dict_c, []),
                           (build_dict_d, []), (build_dict_e, ["--no-kindle-limits"]),
                           (build_dict_f, []), (build_dict_g, []),
                           # KD-H carries a deliberately dead cross-reference,
                           # which the validator is right to flag; the whole
                           # point is to see what the device does with it.
                           (build_dict_h, ["--no-validate"]), (build_dict_i, [])):
        d, opf = builder(out)
        stem = os.path.basename(d)
        target = os.path.join(ship, f"{stem}.mobi")
        run([K, "build", os.path.join(d, opf), "-o", target] + flags)

    print("books")
    # #30: the EPUB 3 spelling, and nothing else, has to carry the cover.
    d, opf = book_source(out, "book-coverimage", "KB coverimage epub3",
                         "kindling-device-coverimage", "cover-image")
    run([K, "build", os.path.join(d, opf), "-o", os.path.join(ship, "book-coverimage.mobi"),
         "--legacy-mobi"])

    # #35: build with no --doc-type at all, then stamp it after the fact.
    d, opf = book_source(out, "book-doctype", "KB doctype rewrite",
                         "kindling-device-doctype", "coverimage")
    plain = os.path.join(out, "src", "book-doctype-plain.mobi")
    run([K, "build", os.path.join(d, opf), "-o", plain, "--legacy-mobi"])
    run([K, "rewrite-metadata", plain, "--doc-type", "ebok",
         "-o", os.path.join(ship, "book-doctype.mobi")])

    # #34's remaining half: the comic path flattens, the thumbnail path does not.
    d, opf = book_source(out, "book-alphacover", "KB alpha cover",
                         "kindling-device-alphacover", "coverimage", cover_name="cover.png")
    run([K, "build", os.path.join(d, opf), "-o", os.path.join(ship, "book-alphacover.mobi"),
         "--legacy-mobi"])

    # #43 / #26: identical books whose covers differ only in JPEG segment order.
    # Both go out as EBOK so each gets a lock screen and a library tile, which is
    # the only place the difference could show up.
    for name, exif in (("book-jfifcover", False), ("book-exifcover", True)):
        d, opf = book_source(out, name, f"KB {name.split('-')[1]}",
                             f"kindling-device-{name.split('-')[1]}", "coverimage",
                             exif_first=exif)
        run([K, "build", os.path.join(d, opf), "-o", os.path.join(ship, f"{name}.mobi"),
             "--legacy-mobi", "--doc-type", "ebok"])

    # #50: footnote markers, their back-links, and a same-document jump.
    # Dual format so both link writers ship in one file: a modern Kindle reads
    # the KF8 half, and the MOBI6 half is there for a pre-2012 device if one
    # turns up. This book must build with no unresolved-link warning at all.
    d, opf = footnote_book(out)
    run([K, "build", os.path.join(d, opf), "-o", os.path.join(ship, "book-footnotes.mobi"),
         "--legacy-mobi"])

    # #45: replace a book's cover after the fact and check the library tile
    # follows it. Built with a plainly-coloured cover, then rewritten to a
    # different colour, so a photo of the shelf answers it. The tile only
    # updates once `kindling thumbnail` installs it, which copy.sh does.
    d, opf = book_source(out, "book-covertile", "KB cover tile",
                         "kindling-device-covertile", "coverimage")
    tile_plain = os.path.join(out, "src", "book-covertile-plain.mobi")
    run([K, "build", os.path.join(d, opf), "-o", tile_plain,
         "--legacy-mobi", "--doc-type", "ebok"])
    green = os.path.join(out, "src", "cover-green.jpg")
    solid_jpeg(green, 600, 800, (0, 160, 0), label="GREEN")
    run([K, "rewrite-metadata", tile_plain, "--cover", green,
         "-o", os.path.join(ship, "book-covertile.mobi")])

    d, opf = probe_book(out)
    run([K, "build", os.path.join(d, opf), "-o", os.path.join(ship, "probe.mobi"),
         "--legacy-mobi"])

    print("comics")
    # Titled explicitly: without --title every comic lands in the library as
    # "Comic", so a round with three of them gives three identical entries
    # and no way to tell which check you are looking at.
    # #37: every page is smaller than the paperwhite profile box (1072x1448).
    small = make_cbz(out, "comic-small", comic_page, 10, 600, 800, label="source 600x800")
    # --crop 0 so the shipped pixels match the size printed on the page, which
    # makes a photo of the screen self-documenting.
    run([K, "comic", small, "-o", os.path.join(ship, "comic-small.mobi"), "--crop", "0",
         "--title", "KC-small 600x800 source"])
    # Control: the same pages well above the profile, which must still shrink.
    big = make_cbz(out, "comic-big", comic_page, 6, 2400, 3200, label="source 2400x3200")
    run([K, "comic", big, "-o", os.path.join(ship, "comic-big.mobi"),
         "--title", "KC-big 2400x3200 control"])
    # #34: transparent ground, normal page aspect so this takes the flattened path.
    alpha = make_cbz(out, "comic-alpha", alpha_page, 6, 1000, 1400)
    # --crop 0 is mandatory here: the default margin crop trims the transparent
    # ground away entirely, leaving only the opaque art and proving nothing.
    run([K, "comic", alpha, "-o", os.path.join(ship, "comic-alpha.mobi"), "--crop", "0",
         "--title", "KC-alpha transparent ground"])
    # #58: the KF7 half of a --legacy-mobi comic. Its layout used to live in
    # CSS, which a MOBI6 reader does not apply, so no page was centred there.
    # The pages are deliberately NARROWER than the profile box so that a
    # centred page and a flush-left one are told apart at a glance.
    narrow = make_cbz(out, "comic-legacy", comic_page, 6, 700, 1400,
                      label="narrow, must be CENTRED")
    run([K, "comic", narrow, "-o", os.path.join(ship, "comic-legacy.mobi"), "--crop", "0",
         "--legacy-mobi", "--title", "KC-legacy KF7 centring"])

    print("\nbuilt into", ship)
    for f in sorted(os.listdir(ship)):
        print(f"  {os.path.getsize(os.path.join(ship, f)):>9,}  {f}")


if __name__ == "__main__":
    main()
