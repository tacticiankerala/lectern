// The frame an HTML page or a PDF shows in: it fills the document pane in place of the document
// (`#lx-doc-pane.framed`). The frame is in the page before its `src` is set, as WebView2 gives the
// first document the frame's colour scheme only then. Every show loads a new URL (`?v=<n>`): only
// a reload, never a change of hash, takes a PDF to a page or shows a change on disk.
//
// A PDF's frame has no sandbox, which WebView2's viewer won't run in. A page's always has one that
// allows scripts and never the same origin, and every response of the page protocol but a PDF's
// carries a sandboxing CSP of its own.
import type { DocPayload } from "./generated/DocPayload";

/** Where a PDF opens: no reading position is kept in one. */
const PDF_START = "#page=1";

export class FrameView {
  private frame: HTMLIFrameElement | null = null;
  /** Bumped by every show, so each loads a new URL. */
  private version = 0;

  constructor(private readonly pane: HTMLElement) {}

  /** Whether a frame is showing. */
  get shown(): boolean {
    return this.frame !== null;
  }

  /** Shows `doc`, a page or a PDF, from `url` (its `frameUrl`) in a new frame. */
  show(doc: DocPayload, url: string): void {
    this.close();
    const frame = document.createElement("iframe");
    frame.className = "lx-frame";
    frame.title = doc.title;
    if (doc.kind !== "pdf") {
      frame.setAttribute("sandbox", "allow-scripts");
    }
    this.pane.classList.add("framed");
    this.pane.append(frame);
    this.frame = frame;
    this.version += 1;
    const src = `${url}?v=${String(this.version)}`;
    frame.src = doc.kind === "pdf" ? src + PDF_START : src;
  }

  /** Takes the frame away, so the document shows again. */
  close(): void {
    this.frame?.remove();
    this.frame = null;
    this.pane.classList.remove("framed");
  }
}
