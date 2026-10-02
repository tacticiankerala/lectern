---
status: parked
started: 2025-11-20
updated: 2026-02-14
---

# Tide Clock

A watch face that shows the next high and low tide at the nearest harbour.

Parked until the watch platform lets faces refresh in the background. The prototype works, but only updates while its app is open.

## Where it got to

- [x] Tide predictions from harmonic constants, within about ten minutes
- [x] A complication for the next high tide
- [ ] Background refresh, waiting on the platform

```bash
# Regenerate the bundled table of harbours
scripts/harbours.sh --region north-atlantic > data/harbours.json
```
