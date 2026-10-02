// Back and forward through the documents opened (spec §6). Each entry is a document and the
// reading position it was left at, captured just before navigating away, so going back returns to
// that spot.
import { samePath } from "./dom";
import type { SavedPosition } from "./generated/SavedPosition";

export interface HistoryEntry {
  path: string;
  position: SavedPosition | null;
}

const DEFAULT_LIMIT = 100;

export class History {
  private readonly behind: HistoryEntry[] = [];
  private ahead: HistoryEntry[] = [];

  constructor(private readonly limit = DEFAULT_LIMIT) {}

  /**
   * Records the entry being left for a new document. The forward entries go; a push of the path
   * already on top only updates its position.
   */
  push(e: HistoryEntry): void {
    this.ahead = [];
    const top = this.behind[this.behind.length - 1];
    if (top && samePath(top.path, e.path)) {
      this.behind[this.behind.length - 1] = e;
      return;
    }
    this.behind.push(e);
    if (this.behind.length > this.limit) {
      this.behind.shift();
    }
  }

  /**
   * The entry to go back to, leaving `current` ahead (unless there is none, on the welcome
   * screen); null when there is nowhere to go.
   */
  back(current: HistoryEntry | null): HistoryEntry | null {
    const e = this.behind.pop();
    if (!e) {
      return null;
    }
    if (current) {
      this.ahead.push(current);
    }
    return e;
  }

  /** The entry to go forward to, leaving `current` behind (if any); null when there is none. */
  forward(current: HistoryEntry | null): HistoryEntry | null {
    const e = this.ahead.pop();
    if (!e) {
      return null;
    }
    if (current) {
      this.behind.push(current);
    }
    return e;
  }

  /** A copy to change tentatively, for a travel that may not land. */
  clone(): History {
    const copy = new History(this.limit);
    copy.behind.push(...this.behind);
    copy.ahead = [...this.ahead];
    return copy;
  }

  canBack(): boolean {
    return this.behind.length > 0;
  }

  canForward(): boolean {
    return this.ahead.length > 0;
  }
}
