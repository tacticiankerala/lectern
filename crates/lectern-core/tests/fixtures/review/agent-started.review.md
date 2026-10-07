---
lectern-review: 1
note: tide.md
---
# Review: tide.md

<!-- lectern: review comments on the note, for AI agents and people.
Reply: add a paragraph under the comment starting **Claude (reply):**, with your own name and the kind reply, question, pushback or resolved.
New comment: append "## C<next number> · open · L<line> · <Heading>", then "> exact words from the note", then **Claude (question):** and your text.
Edit nothing else. The quote outranks line numbers; resolved and dismissed comments need nothing. -->

## C1 · open · L5 · Tide sync › Batching
<!-- anchor prefix=" The client uploads readings in " suffix=". Retries Retry with jitter afte" fp="fnv1a64:592e4f21891a6582" n=1 created="2026-10-06T10:00:00Z" -->
> batches of at most 50

**You:** Why 50? Make it a setting.

## C2 · resolved · L9 · Tide sync › Retries
<!-- anchor prefix=" batches of at most 50. Retries " suffix=" after a failed upload." fp="fnv1a64:592e4f21891a6582" n=2 created="2026-10-06T10:05:00Z" -->
> Retry with jitter

**You:** Which jitter formula?

**Claude (resolved):** Full jitter, now spelled out in the note.

## C3 · open · L9 · Retries
> after a failed upload

**Claude (question):** After every failed upload, or only after a timeout? A rejected batch will fail again however long it waits.
