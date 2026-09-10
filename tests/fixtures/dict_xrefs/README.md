# dict_xrefs

A two-file dictionary whose entries link to each other, built to catch a
resolver that looks fragments up book-wide instead of per source file.

Both files define `<a id="dup">`. `#dup` in file 1 must reach file 1's anchor
and `#dup` in file 2 must reach file 2's, which is what kindlegen does with
the same source. A single book-wide table sends one of them to the other
file's anchor: a live link to the wrong definition, which is worse than a
dead one and looks fine in a screenshot.

The rest is the shape reader.dict and PyGlossary produce. Each entry carries
its anchor as `id="hw_<headword>"` on the `<idx:entry>` element, which the
MOBI text strip removes, so nothing in the merged blob carries that name and
the fragment has to fall back to the entry's own start offset. `#hw_nosuch`
in file 2 names a headword the dictionary does not have and must stay inert.
