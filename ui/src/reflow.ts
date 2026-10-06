// Keeping the reader's place while the layout moves under it: placing an element and holding it
// there while lazy layout settles, and holding the reading spot while the text reflows (a settings
// change moves it) or the window resizes (focus mode going to or from full screen).

/** How long focus mode holds the reader's place while the window goes to or from full screen. */
const HOLD_MS = 1500;
/** The hold ends once the place has held still for this many frames after a resize. */
const HOLD_STABLE_FRAMES = 3;
/** Placing ends once the target has held still (within 2 px) for this many frames... */
const STABLE_FRAMES = 3;
const STABLE_PX = 2;
/** ...or after this long. */
const PLACE_MS = 1000;
/** Input that means the reader is scrolling: placing and holding stop rather than fight it. */
export const READER_INPUT = ["wheel", "keydown", "touchstart", "pointerdown"] as const;

/**
 * Stops the placing in progress in each scroller, so only the latest placement holds. The modules
 * loaded on first use are bundled apart with their own copies of what they import, so they reach
 * this through their host, never by importing it.
 */
const placing = new WeakMap<HTMLElement, () => void>();

/**
 * Scrolls `scroller` so `el`'s top sits `at` pixels below its top, and keeps it there. Blocks that
 * content-visibility kept at an estimated height take their real one as they come near the
 * viewport, which moves `el` by up to thousands of pixels a frame or two later; so the position
 * is corrected every frame until `el` has held still for a few frames, for at most a second, and
 * not once the reader scrolls. A newer placement in the same scroller replaces this one.
 */
export function placeAt(scroller: HTMLElement, el: HTMLElement, at: number): void {
  placing.get(scroller)?.();
  const correct = (): number => {
    const delta = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top - at;
    if (Math.abs(delta) > 0.5) {
      scroller.scrollTop += delta;
    }
    return delta;
  };
  correct();
  const started = performance.now();
  let stable = 0;
  let frame = 0;
  const stop = (): void => {
    cancelAnimationFrame(frame);
    for (const type of READER_INPUT) {
      window.removeEventListener(type, stop, true);
    }
    if (placing.get(scroller) === stop) {
      placing.delete(scroller);
    }
  };
  const tick = (): void => {
    if (!el.isConnected || performance.now() - started > PLACE_MS) {
      stop();
      return;
    }
    stable = Math.abs(correct()) <= STABLE_PX ? stable + 1 : 0;
    if (stable >= STABLE_FRAMES) {
      stop();
      return;
    }
    frame = requestAnimationFrame(tick);
  };
  for (const type of READER_INPUT) {
    window.addEventListener(type, stop, { capture: true, passive: true });
  }
  frame = requestAnimationFrame(tick);
  placing.set(scroller, stop);
}

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

  /** Ends the hold in progress, if any, leaving the place where it is now. */
  stop(): void {
    this.stopHold?.();
  }
}
