// Back and forward through the documents opened (spec §6). Each entry is a document and the
// reading position it was left at, captured just before navigating away, so going back returns to
// that spot. A mark records the spot before something else moves within the document (an agent's
// show), so Back returns there too; it stays a mark as the reader goes back to it and forward from
// it again.
import { samePath } from "./dom";
import type { SavedPosition } from "./generated/SavedPosition";

export interface HistoryEntry {
  path: string;
  position: SavedPosition | null;
  /** Recorded before a move within the document: a later push of its path never replaces it. */
  mark?: boolean;
}

const DEFAULT_LIMIT = 100;

export class History {
  private readonly behind: HistoryEntry[] = [];
  private ahead: HistoryEntry[] = [];
  /** The reader went back or forward to a mark: the spot they leave by going on is still one. */
  private atMark = false;

  constructor(private readonly limit = DEFAULT_LIMIT) {}

  /**
   * Records the entry being left for a new document. The forward entries go; a push of the path
   * already on top only updates its position, unless the top is a mark.
   */
  push(e: HistoryEntry): void {
    this.ahead = [];
    this.atMark = false;
    const top = this.behind[this.behind.length - 1];
    if (top && !top.mark && samePath(top.path, e.path)) {
      this.behind[this.behind.length - 1] = e;
      return;
    }
    this.add(e);
  }

  /**
   * Records where the reader is in a document before something else moves within it, as a mark.
   * The forward entries go; a mark of the spot already on top adds nothing.
   */
  mark(e: HistoryEntry): void {
    this.ahead = [];
    this.atMark = false;
    const top = this.behind[this.behind.length - 1];
    if (
      top &&
      samePath(top.path, e.path) &&
      JSON.stringify(top.position) === JSON.stringify(e.position)
    ) {
      return;
    }
    this.add({ ...e, mark: true });
  }

  private add(e: HistoryEntry): void {
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
      this.ahead.push(this.left(current));
    }
    this.atMark = e.mark === true;
    return e;
  }

  /** The entry to go forward to, leaving `current` behind (if any); null when there is none. */
  forward(current: HistoryEntry | null): HistoryEntry | null {
    const e = this.ahead.pop();
    if (!e) {
      return null;
    }
    if (current) {
      this.behind.push(this.left(current));
    }
    this.atMark = e.mark === true;
    return e;
  }

  /** The spot being left by going back or forward: a mark still, when it was reached as one. */
  private left(current: HistoryEntry): HistoryEntry {
    return this.atMark ? { ...current, mark: true } : current;
  }

  /** A copy to change tentatively, for a travel that may not land. */
  clone(): History {
    const copy = new History(this.limit);
    copy.behind.push(...this.behind);
    copy.ahead = [...this.ahead];
    copy.atMark = this.atMark;
    return copy;
  }

  canBack(): boolean {
    return this.behind.length > 0;
  }

  canForward(): boolean {
    return this.ahead.length > 0;
  }
}
