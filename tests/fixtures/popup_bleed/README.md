# Popup scroll-through repro (bleed)

Minimal on-device demonstration for the lookup-popup entry-boundary fix.
Five common English words; `house` has a multi-screen article so the popup
must be scrolled, and `xylophone` — an entirely unrelated headword — is its
physical neighbour in the single text record.

Build the same source on `main` and on the fix branch:

```
kindling-cli build content.opf -o before.mobi    # main:      </div><hr/> between entries
kindling-cli build content.opf -o after.mobi     # fix branch: </div><hr/><mbp:pagebreak/>
```

Both files are one text record, 1460/1540 bytes, spans byte-identical
(`house` 736, `xylophone` 136) — the separator is the only difference.

On device, in any English book:

1. Sideload `before.mobi`, select the word **house**, scroll the popup to the
   bottom. The last screen reaches past the article and shows **xylophone**'s
   headword (and depending on remaining space, its definition) under
   *house* — the next entry bleeding into the lookup popup.
2. Remove it, sideload `after.mobi`, repeat. The popup now ends at *house*'s
   own last line; scrolling cannot leave the entry, because the popup's page
   box ends at the `<mbp:pagebreak/>`.

Geometry caveat (why `house` is long and the neighbour is unrelated): the
popup's reach past the span is small and line-wrap dependent, so the probe
article must be long enough to force scrolling and the visible neighbour must
be unmistakably not-a-match (the original report was Estonian
`patt` → `tarbeelektroonika`). If a given firmware shows only the neighbour's
headword line, that is still the bug — one line of an unrelated entry is
reachable at all.
