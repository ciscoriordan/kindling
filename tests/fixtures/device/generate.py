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
to drive all five dictionaries.

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
# question from across the room. `u` is the control: it sits before the escaped
# colon and must always apply. `i` sits after it and is the signal.
CONTROL_RULE = "u { font-size: 260%; font-weight: bold; }"
SIGNAL_RULE = "i { font-size: 260%; font-weight: bold; }"
TRAP_RULE = "idx\\:orth { display: block; }"

STYLE_PROBE_BODY = (
    "<p><u>UNDER</u> <i>ITAL</i> plain</p>"
    "<p>UNDER and ITAL must both be large. If only UNDER is large, "
    "the rule after the escaped colon was discarded.</p>"
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

def entry(orth, body, visible=True):
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
    return (f'<idx:entry name="default" scriptable="yes">{orth_tag}{body}'
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
DUP_WORDS = [f"zdup{n:02d}" for n in range(1, 31)]
NBSP_HEAD = f"znbspalpha{NBSP}znbspbeta"
NOLIMIT_WORDS = [f"znolimit{n:02d}" for n in range(1, 11)]
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
        "all five appear in that list.</p>",
        section(1, "1. Style, in all four dictionaries (issues 39 and 40)",
                "Look up zstyle, then run it through KD-A, KD-B, KD-C and KD-D in turn. "
                "In each one, UNDER and ITAL must both be much larger than the word plain. "
                "If UNDER is large and ITAL is not, that dictionary lost the rule after the "
                "escaped colon. If neither is large, that dictionary shipped no stylesheet.",
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
        "<h2>5. Controls</h2><p>These are ordinary entries in every dictionary. If they "
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
                           (build_dict_d, []), (build_dict_e, ["--no-kindle-limits"])):
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

    d, opf = probe_book(out)
    run([K, "build", os.path.join(d, opf), "-o", os.path.join(ship, "probe.mobi"),
         "--legacy-mobi"])

    print("comics")
    # #37: every page is smaller than the paperwhite profile box (1072x1448).
    small = make_cbz(out, "comic-small", comic_page, 10, 600, 800, label="source 600x800")
    # --crop 0 so the shipped pixels match the size printed on the page, which
    # makes a photo of the screen self-documenting.
    run([K, "comic", small, "-o", os.path.join(ship, "comic-small.mobi"), "--crop", "0"])
    # Control: the same pages well above the profile, which must still shrink.
    big = make_cbz(out, "comic-big", comic_page, 6, 2400, 3200, label="source 2400x3200")
    run([K, "comic", big, "-o", os.path.join(ship, "comic-big.mobi")])
    # #34: transparent ground, normal page aspect so this takes the flattened path.
    alpha = make_cbz(out, "comic-alpha", alpha_page, 6, 1000, 1400)
    # --crop 0 is mandatory here: the default margin crop trims the transparent
    # ground away entirely, leaving only the opaque art and proving nothing.
    run([K, "comic", alpha, "-o", os.path.join(ship, "comic-alpha.mobi"), "--crop", "0"])

    print("\nbuilt into", ship)
    for f in sorted(os.listdir(ship)):
        print(f"  {os.path.getsize(os.path.join(ship, f)):>9,}  {f}")


if __name__ == "__main__":
    main()
