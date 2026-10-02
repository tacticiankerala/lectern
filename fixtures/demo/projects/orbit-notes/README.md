---
status: active
started: 2026-06-02
updated: 2026-09-22
platforms: [iOS, Android, web]
---

# Orbit Notes

A calm notes app that works the same with or without a signal. Notes live on the device first and sync when they can.

## Now

Offline sync is the big piece this quarter: [[offline-sync|the plan]], and the [[2026-09-18-sync-spike|spike]] that shaped it.

## Next

- [ ] Shared notebooks, once sync has settled
- [ ] A home-screen widget for today's note
- [ ] Export to Markdown, one file per note

## Stack

| Layer | Choice |
| --- | --- |
| API | Rails 8 and Postgres |
| Web | React 19 and Vite |
| Mobile | React Native, with SQLite on the device |
