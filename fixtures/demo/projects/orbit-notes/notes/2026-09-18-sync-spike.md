# Sync spike

#sync #spike

Two days to find out whether a plain change log is enough, before committing to [[offline-sync|the full plan]].

## What we tried

- **Last write wins, per note.** Simple, and it loses text whenever two devices edit offline. Ruled out.
- **A change log with a cursor per device.** One indexed query to catch up, and conflicts are easy to spot because every edit carries its base version.
- **A CRDT per note.** It merges everything, but triples the payload and makes the server hard to reason about. Worth another look for shared notebooks.

## Numbers

Catching up after a week offline, about 1,200 changes, took **180 ms** on a mid-range phone. Most of that was applying the edits to SQLite.

## Decision

Go with the change log. The details are in [[offline-sync#Task 2: The sync endpoint|the endpoint section]] of the plan.

Related: [[orbit-notes/README|Orbit Notes]] itself, and [[seedbed/README|Seedbed]], whose offline map tiles could reuse the same queue.
