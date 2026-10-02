---
name: offline-sync
project: Orbit Notes
started: 2026-09-14
tags: [sync, offline]
---

# Offline sync

> Notes should open instantly and save without a signal. When the phone finds the network again, edits merge quietly and nobody loses a sentence.

**Goal:** every note can be read and edited offline, and changes from all of a person's devices converge within a few seconds of reconnecting.

**Approach:** the server keeps an append-only change log per account. Devices queue their edits locally, push them in order, and pull everything after their last cursor. Conflicting edits to the same note keep both versions and ask the person once.

## Overview

| Piece | Where | Status |
| --- | --- | --- |
| Change log | `api/app/models/change.rb` | ✅ Done |
| Sync endpoint | `api/app/controllers/sync_controller.rb` | In review |
| Device queue | `web/src/sync/queue.ts` | In progress |
| Conflict banner | `web/src/components/ConflictBanner.tsx` | Not started |
| Soak test | `scripts/sync-soak.sh` | Not started |

## Task 1: Record every change

Each save writes a row to `changes`, so a device that has been away can catch up with one query.

- [x] Add the `changes` table, indexed on `(account_id, id)`
- [x] Record a change in the same transaction as the note update
- [x] Backfill one change per existing note

```ruby
class Change < ApplicationRecord
  belongs_to :account
  belongs_to :note

  scope :after, ->(cursor) { where(id: (cursor + 1)..).order(:id) }

  def self.record!(note, device:)
    create!(
      account: note.account,
      note:,
      device_id: device.id,
      body: note.body,
      version: note.lock_version
    )
  end
end
```

## Task 2: The sync endpoint

One endpoint does both directions: the device sends what it has queued, then receives everything after its cursor.

- [x] `POST /sync` accepts up to 200 queued edits
- [x] Reject an edit whose base version is older than the note's, returning the server copy
- [ ] Page responses at 500 changes, with `more: true`

```ruby
class SyncController < ApplicationController
  PAGE = 500

  def create
    cursor = params.fetch(:cursor, 0).to_i
    results = params.require(:edits).map do |edit|
      Notes::ApplyEdit.call(current_account, edit)
    end
    changes = current_account.changes.after(cursor).limit(PAGE)

    render json: {
      applied: results.select(&:applied?).map(&:id),
      conflicts: results.select(&:conflict?).map(&:to_h),
      changes: changes.as_json(only: %i[id note_id body version]),
      cursor: changes.last&.id || cursor,
      more: changes.size == PAGE
    }
  end
end
```

## Task 3: Queue edits on the device

Edits go to IndexedDB first, and the queue drains whenever the browser says it is online.

- [x] Store queued edits with their base version
- [ ] Drain on `online`, on focus, and every 30 seconds
- [ ] Back off after a failed push: 2 s, 4 s, 8 s, capped at a minute

```tsx
export function useSyncStatus(): SyncStatus {
  const [status, setStatus] = useState<SyncStatus>({ state: "idle", pending: 0 });

  useEffect(() => {
    const drain = () => queue.drain().then(setStatus);
    window.addEventListener("online", drain);
    const timer = setInterval(drain, 30_000);
    drain();
    return () => {
      window.removeEventListener("online", drain);
      clearInterval(timer);
    };
  }, []);

  return status;
}
```

## Task 4: Show conflicts gently

When two devices edit the same note offline, nothing is thrown away. The note keeps the newer text and offers the other version once.

- [ ] A banner at the top of the note, never a modal
- [ ] "Keep both" appends the other version under a divider
- [ ] Dismissing the banner keeps the version on screen

```tsx
export function ConflictBanner({ theirs, onKeepBoth, onDismiss }: Props) {
  return (
    <aside className="conflict-banner" role="status">
      <p>
        This note also changed on <strong>{theirs.deviceName}</strong> while you
        were offline.
      </p>
      <button onClick={onKeepBoth}>Keep both versions</button>
      <button className="quiet" onClick={onDismiss}>
        Keep this one
      </button>
    </aside>
  );
}
```

## Task 5: Soak test

Fifty simulated devices edit the same twenty notes for ten minutes, dropping offline at random.

- [ ] Run against staging every night
- [ ] Fail if any device ends with a note body that differs from the server's

```bash
#!/usr/bin/env bash
set -euo pipefail

devices=${DEVICES:-50}
minutes=${MINUTES:-10}

for i in $(seq 1 "$devices"); do
  bin/fake-device --account soak --seed "$i" --minutes "$minutes" --flaky &
done
wait

bin/rails runner 'Soak.verify!(account: "soak")'
```

## Rollout

> [!TIP]
> Sync ships behind the `offline_sync` flag. Turn it on for the studio first, then for 10% of accounts for a week, watching the conflict rate on the dashboard.

1. The studio only, from 6 October
2. 10% of accounts, once the soak test has passed five nights in a row
3. Everyone, with a short note in the changelog

## Open questions

- How long do we keep the change log? Ninety days covers a long holiday; a device away for longer can start from a full download.
- Should "Keep both" be the default on mobile, where a banner is easy to miss?
- Do attachments need their own queue, or can they ride along with the note edit?[^attachments]

[^attachments]: Probably their own: photos are large, and a note shouldn't wait for one to upload.
