# Issue policy

The tracker is the list of what is currently wrong with kindling and what is
planned. If something is broken or missing and it is not here, it is not being
tracked. Commit messages and release notes say what happened; issues say what
is still outstanding. Both exist and neither replaces the other.

## What gets an issue

One issue per thing a user can observe: a file that will not open, a lookup
that returns the wrong entry, a cover that does not appear, a flag that does
the wrong thing. Not one per task it would take to fix.

Planned work belongs here too, not just defects. If a fix is deliberately
deferred (a helper that would be nice, a rework with no verified target), it
gets an issue rather than living in someone's head.

Work that starts and finishes in the same sitting does not get an issue first.
That is churn; the commit is the record.

Before filing, search open and closed issues. Closed ones matter: several
questions here have been settled with device evidence and should not be
relitigated without new evidence.

## Closing

Close when the reported behavior is fixed **on a real device**, not when the
code lands. kindling's bugs are overwhelmingly device-behavior bugs, and the
history has several fixes that were structurally correct and wrong in practice.
An unverified fix leaves the issue open with a note saying what needs checking.

The closing comment says what makes it true: which device, which firmware, and
what was observed. "Fixed in 0.29.2" is not a closing comment.

Close as not-planned when the answer is that kindling will not do the thing,
and say why in enough detail that the next person does not reopen it. Issue #22
(Korean lookup) is the model: the verdict, the evidence, and what to use
instead.

## Labels

Deliberately few, because a label nobody sets is worse than no label.

- `bug` - kindling produces something wrong
- `device-verified` - the behavior was reproduced or the fix confirmed on
  hardware, with the model and firmware in a comment
- `needs-device-check` - a fix is believed correct but nobody has opened it on
  a Kindle yet
- `upstream` - the cause is Kindle firmware or another tool, not kindling
- `not-planned` - closed by decision rather than by fix

## Reporting

A file that reproduces the problem is worth more than a description of it.
Include the kindling version, the device and firmware if it is a device
behavior, and whether it happens with kindlegen or calibre output too.
