---
lectern-review: 1
note: 2026-09-14-offline-sync.md
---
# Review: 2026-09-14-offline-sync.md

<!-- lectern: review comments on the note, for AI agents and people.
Reply: add a paragraph under the comment starting **Claude (reply):**, with your own name and the kind reply, question, pushback or resolved.
New comment: append "## C<next number> · open · L<line> · <Heading>", then "> exact words from the note", then **Claude (question):** and your text.
Edit nothing else. The quote outranks line numbers; resolved and dismissed comments need nothing. -->

## C1 · open · L88 · Offline sync › Task 3: Queue edits on the device
<!-- anchor prefix="d edits with their base version " suffix=" Back off after a failed push: 2" fp="fnv1a64:de88e339f0ffa9c4" n=1 created="2026-09-29T09:12:00Z" -->
> Poll the server every five seconds while edits are waiting

**You:** Polling every five seconds will flatten a phone battery. Can the queue wait for the browser's `online` event instead?

**Claude (reply):** Agreed. I rewrote the task: the queue now drains on `online`, on focus, and every 30 seconds as a fallback.

## C2 · resolved · L137 · Offline sync › Task 5: Soak test
<!-- anchor prefix="\u003c/aside\u003e ); } Task 5: Soak test " suffix=", dropping offline at random. Ru" fp="fnv1a64:319545415f39e179" n=2 created="2026-09-30T14:20:00Z" -->
> Fifty simulated devices edit the same twenty notes for ten minutes

**You:** Is ten minutes long enough to catch a slow drift between devices?

**Claude (resolved):** Ten minutes gives each device about forty offline spells at the soak script's rate, and the run repeats every night. Left as is.

## C3 · open · L57 · Offline sync › Task 2: The sync endpoint
<!-- anchor prefix="ng after its cursor. POST /sync " suffix=" Reject an edit whose base versi" fp="fnv1a64:319545415f39e179" n=1 created="2026-10-01T10:05:00Z" -->
> accepts up to 200 queued edits

**You:** Why 200? A week offline on a busy phone could queue more than that.

**Claude (question):** Should the cap be per device or per account? Per device keeps one busy phone from crowding out the others.
