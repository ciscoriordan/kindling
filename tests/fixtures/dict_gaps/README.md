# dict_gaps

A dictionary whose files carry text outside `<idx:entry>`, all of which used
to be dropped in silence with exit 0 (issue #42).

`content.html` has every shape the issue names: a letter heading before the
first entry, a usage note between two entries, an `<idx:entry>` the parser
rejects because it has no `<idx:orth>`, a second letter heading, and a
closing credits paragraph after the last entry.

`preface.html` is the case with nowhere to hang its text: it carries
`<idx:entry>` markup but yields no usable entry, so there is no entry to
attach a run to and the whole file would otherwise be skipped as "a
dictionary file".

The two things to hold on to are that none of that text is lost, and that the
three real entries still index to their own headwords rather than to the
heading or the note in front of them. A run that steals an entry's anchor is
issue #27 with a new cause, and it looks like a working dictionary until you
tap a word.
