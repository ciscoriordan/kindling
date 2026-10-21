#!/usr/bin/env python3
"""Generate reproducible device-check files for Ukrainian dictionary lookup."""
import argparse
import html
from pathlib import Path
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def package(title, identifier, dictionary):
    metadata = """<x-metadata><DictionaryInLanguage>uk</DictionaryInLanguage>
<DictionaryOutLanguage>uk</DictionaryOutLanguage><DefaultLookupIndex>default</DefaultLookupIndex></x-metadata>""" if dictionary else ""
    return f'''<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>{title}</dc:title><dc:creator>kindling</dc:creator><dc:language>uk</dc:language>
<dc:identifier id="BookId">{identifier}</dc:identifier>{metadata}</metadata>
<manifest><item id="content" href="content.html" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="content"/></spine></package>'''


def generate(binary, output):
    output.mkdir(parents=True, exist_ok=True)
    version = subprocess.check_output([str(binary), "--version"], text=True).strip()
    cases = {"A": "сум’яття", "B": "сум'яття", "C": "сумʼяття", "D": "сумяття"}
    for name, label in cases.items():
        source = output / "sources" / name
        source.mkdir(parents=True, exist_ok=True)
        source.joinpath("content.opf").write_text(package(f"Ukrainian lookup {name}", f"kindling-uk-lookup-{name}", True), encoding="utf-8")
        entries = "".join(f'<idx:entry><idx:orth value="{html.escape(word, quote=True)}"/><b>{html.escape(word)}</b><p>Dictionary {name}: {description}.</p></idx:entry>'
                          for word, description in [("одразу", "control"), (label, "apostrophe check")])
        source.joinpath("content.html").write_text('<html xmlns:idx="http://www.mobipocket.com/idx"><head><title>Lookup probe</title></head><body>' + entries + '</body></html>', encoding="utf-8")
        for strict in [False, True]:
            destination = output / f"dictionary-{name}{'-strict' if strict else ''}.mobi"
            subprocess.run([str(binary), "build", str(source / "content.opf"), "--no-validate", *(["--strict-accents"] if strict else []), "-o", str(destination)], check=True)
    source = output / "sources" / "book"
    source.mkdir(parents=True, exist_ok=True)
    source.joinpath("content.opf").write_text(package("Ukrainian apostrophe lookup check", "kindling-uk-lookup-book", False), encoding="utf-8")
    source.joinpath("content.html").write_text('<html><head><title>Ukrainian lookup</title></head><body>' + ''.join(f'<p>{html.escape(word)}</p>' for word in ["одразу", "сумяття", "сум’яття", "сум'яття", "сумʼяття"]) + '</body></html>', encoding="utf-8")
    subprocess.run([str(binary), "build", str(source / "content.opf"), "--no-validate", "-o", str(output / "lookup-book.azw3")], check=True)
    output.joinpath("README.txt").write_text(f'''Ukrainian dictionary lookup device check
Built with {version}.

Install lookup-book.azw3 and ONE dictionary at a time over USB. Select that
dictionary in the lookup popup. Remove it before installing the next one;
default and strict builds of the same dictionary share source identifiers.

In every dictionary, одразу is the positive control. Look up each of the four
сумяття spellings in the book. They should reach the apostrophe-check entry in
all eight dictionaries. Record the device, firmware, selected dictionary,
query spelling, and displayed definition. Test both default and strict files.

A stores U+2019, B U+0027, C U+02BC, and D no apostrophe. These files test index
lookup and the reader's selection path together. Native search has been checked
in extracted firmware 5.20.1; this does not establish behavior on a real device.

Sources are included. Rebuild from the repository with:
cargo build --bin kindling-cli
python3 scripts/ukrainian_lookup_probes.py

Tracking: https://github.com/ciscoriordan/kindling/issues/71
          https://github.com/ciscoriordan/kindling/issues/72
''', encoding="utf-8")
    archive = output / "ukrainian-lookup-probes.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
        for path in sorted(output.rglob("*")):
            if path.is_file() and path != archive:
                bundle.write(path, path.relative_to(output))
    print(archive)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/kindling-cli")
    parser.add_argument("--output", type=Path, default=ROOT / "target/ukrainian-lookup-probes")
    args = parser.parse_args()
    generate(args.binary.resolve(), args.output.resolve())
