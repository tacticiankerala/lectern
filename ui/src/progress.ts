// The reading progress bar: how far the document pane has scrolled, as `scaleX` on #lx-progress.

/**
 * Tracks `scroller`'s scroll on `bar` with a passive listener, updating at most once a frame.
 * Returns the update, for when the content changes without a scroll.
 */
export function trackProgress(scroller: HTMLElement, bar: HTMLElement): () => void {
  let queued = false;
  const update = (): void => {
    queued = false;
    const max = scroller.scrollHeight - scroller.clientHeight;
    const fraction = max > 0 ? Math.min(1, Math.max(0, scroller.scrollTop / max)) : 0;
    bar.style.transform = `scaleX(${fraction.toFixed(4)})`;
  };
  const schedule = (): void => {
    if (!queued) {
      queued = true;
      requestAnimationFrame(update);
    }
  };
  scroller.addEventListener("scroll", schedule, { passive: true });
  return schedule;
}
