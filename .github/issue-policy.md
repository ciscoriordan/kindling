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

Write the closing comment first, then close. If writing it turns up a caveat
that starts "needs a device check", that is the issue telling you it is not
ready. #40 and #41 were both closed and then given a closing comment admitting
they were premature, minutes apart, which is how this rule got written down.

There is one carve-out, and it is narrow: a defect that only appears at a scale
nobody here can build. #32 needed a 266 MB dictionary. Close those on the
arithmetic, say in the comment that no device was involved and why, and leave
the door open.

Close as not-planned when the answer is that kindling will not do the thing,
and say why in enough detail that the next person does not reopen it. Issue #22
(Korean lookup) is the model: the verdict, the evidence, and what to use
instead.

## Labels

Deliberately few, because a label nobody sets is worse than no label.

- `bug` - kindling produces something wrong
- `enhancement` - kindling does not do a thing that it should; a feature
  request rather than a defect
- `device-verified` - the behavior was reproduced or the fix confirmed on
  hardware, with the model and firmware in a comment
- `needs-device-check` - a fix is believed correct but nobody has opened it on
  a Kindle yet. This one means a fix exists and is waiting on hardware. Do not
  put it on an issue with no fix in the tree; there is nothing to check.
- `upstream` - the cause is Kindle firmware or another tool, not kindling
- `not-planned` - closed by decision rather than by fix

Nothing else is installed. GitHub's default set was removed rather than left
sitting unused.

## Reporting

A file that reproduces the problem is worth more than a description of it.
Include the kindling version, the device and firmware if it is a device
behavior, and whether it happens with kindlegen or calibre output too.
