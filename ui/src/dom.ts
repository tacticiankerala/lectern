// DOM helpers and the app's layout. Chrome ids carry the `lx-` prefix: document headings get slug
// ids and raw HTML may carry ids, so lookups inside a document are scoped to it (`findById`).

/** The layout inside `#lx-app`, which Task 10 styles and Tasks 11–12 attach to. */
const LAYOUT = `
<header id="lx-header"><nav id="lx-history-nav"></nav><nav id="lx-breadcrumbs"></nav><div id="lx-header-actions"></div></header>
<div id="lx-progress"></div>
<aside id="lx-library" class="sidebar"></aside><div class="resizer" data-for="library"></div>
<main id="lx-doc-pane"><div id="lx-banner" hidden></div><section id="lx-properties" class="props" hidden></section><article id="lx-doc" class="doc"></article></main>
<div class="resizer" data-for="outline"></div><aside id="lx-outline" class="sidebar"></aside>
<div id="lx-overlay-root"></div><div id="lx-toasts" aria-live="polite"></div>`;

export interface Layout {
  app: HTMLElement;
  header: HTMLElement;
  historyNav: HTMLElement;
  breadcrumbs: HTMLElement;
  headerActions: HTMLElement;
  progress: HTMLElement;
  library: HTMLElement;
  docPane: HTMLElement;
  banner: HTMLElement;
  properties: HTMLElement;
  doc: HTMLElement;
  outline: HTMLElement;
  overlayRoot: HTMLElement;
  toasts: HTMLElement;
}

/** Fills `root` (`#lx-app`) with the layout and returns its parts, found before any document. */
export function buildLayout(root: HTMLElement): Layout {
  root.innerHTML = LAYOUT;
  const part = (id: string): HTMLElement => {
    const el = root.querySelector<HTMLElement>(`#${id}`);
    if (!el) {
      throw new Error(`the layout has no #${id}`);
    }
    return el;
  };
  return {
    app: root,
    header: part("lx-header"),
    historyNav: part("lx-history-nav"),
    breadcrumbs: part("lx-breadcrumbs"),
    headerActions: part("lx-header-actions"),
    progress: part("lx-progress"),
    library: part("lx-library"),
    docPane: part("lx-doc-pane"),
    banner: part("lx-banner"),
    properties: part("lx-properties"),
    doc: part("lx-doc"),
    outline: part("lx-outline"),
    overlayRoot: part("lx-overlay-root"),
    toasts: part("lx-toasts"),
  };
}

/** Headings that can be scrolled to. */
export const HEADINGS = "h1[id], h2[id], h3[id], h4[id], h5[id], h6[id]";

/** The element with `id` in the page. Only for chrome, before any document is shown. */
export function byId(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (!el) {
    throw new Error(`no #${id}`);
  }
  return el;
}

function cssEscape(value: string): string {
  if (typeof CSS !== "undefined" && typeof CSS.escape === "function") {
    return CSS.escape(value);
  }
  // jsdom has no CSS.escape; inside a quoted attribute value, these are what need escaping.
  return value.replace(/["\\]/g, "\\$&").replace(/\n/g, "\\a ");
}

/** The element inside `scope` whose id is exactly `id`. */
export function findById(scope: ParentNode, id: string): HTMLElement | null {
  return scope.querySelector<HTMLElement>(`[id="${cssEscape(id)}"]`);
}

/** An element with attributes and children; strings become text nodes, never markup. */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Record<string, string> = {},
  ...children: (Node | string)[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) {
    el.setAttribute(name, value);
  }
  el.append(...children);
  return el;
}

/** Resolves once the current DOM has been painted: two animation frames. */
export function nextPaint(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        resolve();
      });
    });
  });
}

/** Paths compare as on Windows: case-insensitively, either separator. */
export function samePath(a: string, b: string): boolean {
  const key = (p: string) => p.toLowerCase().replaceAll("/", "\\");
  return key(a) === key(b);
}

/** Runs a promise for its effect, logging a failure instead of leaving it unhandled. */
export function quietly(promise: Promise<unknown>): void {
  promise.catch((e: unknown) => {
    console.warn(e);
  });
}
