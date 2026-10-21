These minimal test fonts and their glyph outlines are original fixtures,
covered by the repository's MIT license. They contain a rectangle for `x`,
a space, and a missing-glyph rectangle, and are not reading fonts.

Regenerate them with `python3 tests/fixtures/fonts/generate.py` after installing
`fonttools==4.63.0`. The generator fixes timestamps so repeated runs produce
identical files. Regular, bold, and italic share a family; the fallback fixture
has different legacy and typographic family names. The OTF fixture uses CFF
outlines rather than TrueType outlines.
