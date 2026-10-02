// Keeping the reader's place while the text reflows (a settings change moves it) or the window
// resizes (focus mode going to or from full screen).
import { READER_INPUT } from "./doc-view";

/** How long focus mode holds the reader's place while the window goes to or from full screen. */
const HOLD_MS = 1500;
/** The hold ends once the place has held still for this many frames after a resize. */
const HOLD_STABLE_FRAMES = 3;

/** A reading spot held across a reflow: a block, and how far down it the pane's top falls. */
export interface FlowAnchor {
  block: Element;
  into: number;
}

/**
 * The block of `doc` at the top of `scroller` and how far down it the top falls, as a fraction of
 * its height, to hold across a reflow. Null at the very top.
 */
export function flowAnchor(scroller: HTMLElement, doc: HTMLElement): FlowAnchor | null {
  if (scroller.scrollTop === 0) {
    return null;
  }
  const top = scroller.getBoundingClientRect().top;
  const blocks = doc.children;
  // Blocks come in page order: the first one reaching below the top, by binary search.
  let lo = 0;
  let hi = blocks.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if ((blocks[mid]?.getBoundingClientRect().bottom ?? Infinity) <= top) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  const block = blocks[lo];
  if (!block) {
    return null;
  }
  const rect = block.getBoundingClientRect();
  return { block, into: rect.height > 0 ? (top - rect.top) / rect.height : 0 };
}

/**
 * Scrolls the anchor's block back to where it was at the top, after the text has moved.
 * Returns how far it had moved.
 */
export function keepFlow(scroller: HTMLElement, { block, into }: FlowAnchor): number {
  const rect = block.getBoundingClientRect();
  const moved = rect.top + into * rect.height - scroller.getBoundingClientRect().top;
  if (Math.abs(moved) > 0.5) {
    scroller.scrollTop += moved;
  }
  return moved;
}

/** Holds a reading spot in place through a resize that lands some frames later. */
export class FlowHold {
  /** Ends the hold in progress, if any. */
  private stopHold: (() => void) | null = null;

  constructor(private readonly scroller: HTMLElement) {}

  /**
   * Holds the anchor at the top of the pane while the window goes to or from full screen, which
   * resizes it some frames later: re-corrected every frame until it has held still for a few
   * frames after a resize, or until HOLD_MS, or until the reader scrolls.
   */
  hold(anchor: FlowAnchor): void {
    this.stopHold?.();
    const started = performance.now();
    let resized = false;
    let stable = 0;
    let frame = 0;
    const onResize = (): void => {
      resized = true;
      stable = 0;
    };
    const stop = (): void => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", onResize);
      for (const type of READER_INPUT) {
        window.removeEventListener(type, stop, true);
      }
      if (this.stopHold === stop) {
        this.stopHold = null;
      }
    };
    const tick = (): void => {
      if (!anchor.block.isConnected || performance.now() - started > HOLD_MS) {
        stop();
        return;
      }
      stable = Math.abs(keepFlow(this.scroller, anchor)) <= 1 ? stable + 1 : 0;
      if (resized && stable >= HOLD_STABLE_FRAMES) {
        stop();
        return;
      }
      frame = requestAnimationFrame(tick);
    };
    window.addEventListener("resize", onResize);
    // Added while the key that toggled focus mode is still on its way, these see only later input.
    for (const type of READER_INPUT) {
      window.addEventListener(type, stop, { capture: true, passive: true });
    }
    frame = requestAnimationFrame(tick);
    this.stopHold = stop;
  }
}
