# Wide table

Seven columns, one 800-character cell and one 1,372-character row.

| Area | Owner | Status | Since | Path | Summary | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| library | platform | done | 2025-12-01 | `src/library/scan.rs` | Parallel walk | Fast |
| search | platform | in review | 2026-01-01 | `src/search/index.rs` | The indexer walks every root in parallel, records each file it meets, keeps Markdown files apart for the tree, reads the first few kilobytes of each note for its name and status, and writes a snapshot so the next launch can show the library before the walk finishes. The indexer walks every root in parallel, records each file it meets, keeps Markdown files apart for the tree, reads the first few kilobytes of each note for its name and status, and writes a snapshot so the next launch can show the library before the walk finishes. The indexer walks every root in parallel, records each file it meets, keeps Markdown files apart for the tree, reads the first few kilobytes of each note for its name and status, and writes a snapshot so the next launch can show the library before the walk finishes. | The indexer walks every root in parallel, records each file it meets, keeps Markdown files apart for the tree, reads the first few kilobytes of each note for its name and status, and writes a snapshot so the next launch can show the library before the walk finishes. The indexer walks every root in parallel, records each file it meets, keeps Markdown files apart for the tree, reads the first few kilobytes of each note for its name and status, and writes a snapshot so the next launch can show. |
| render | core | todo | 2026-01-10 | `src/render/mod.rs` | Post-passes | — |

A paragraph after the table, to check the layout recovers.
