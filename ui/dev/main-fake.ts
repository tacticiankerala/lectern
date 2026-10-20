// The UI on the fake backend, for Playwright: `/?open=<path>` stands in for a launch argument.
// `?garden` adds a second workspace, Garden, closed (`?garden=open`: open in another window),
// `?many=<n>` adds n more, "Notebook 1" to "Notebook n", closed with no folders, `?recent=<n>`
// makes the first n fixture documents the recent files, and `?blank` starts a blank window.
// `window.__lxDocText()` reads the page's visible text as the review comments do, to check it
// against core's (`__fake.coreText`); given a selector, the text of that block alone. `window.__lxPage()` is what the HTML page on screen has
// reported through its bridge. `window.__lxViewTop()` is the block at the top of the document
// pane: the source line it starts on, and how far below the pane's top it is.
import { App } from "../src/app";
import { docText } from "../src/comments-text";
import { byId } from "../src/dom";
import {
  FakeBackend,
  GARDEN,
  studio,
  type FakeOptions,
  type FakeWorkspace,
  type Fixtures,
} from "./backend-fake";

/** dev/fixtures.json, inlined by `build.mjs --fake`. */
declare const __LX_FIXTURES__: Fixtures;

/** What the HTML page on screen has reported through its bridge. */
interface PageState {
  ready: boolean;
  text: string;
  fraction: number;
  dark: boolean;
}

declare global {
  interface Window {
    __lxDocText: (block?: string) => string;
    __lxPage: () => PageState | null;
    __lxViewTop: () => { line: number; offset: number } | null;
  }
}

window.__lxDocText = (block?: string) => {
  const doc = document.querySelector(block === undefined ? ".doc" : `.doc ${block}`);
  return doc ? docText(doc) : "";
};

window.__lxViewTop = () => {
  const pane = document.getElementById("lx-doc-pane");
  const doc = document.querySelector(".doc");
  if (!pane || !doc) return null;
  const top = pane.getBoundingClientRect().top;
  for (const block of doc.children) {
    const box = block.getBoundingClientRect();
    if (box.bottom <= top) continue;
    const lined = block.closest("[data-sourcepos]") ?? block.querySelector("[data-sourcepos]");
    const line = Number(/^(\d+):/.exec(lined?.getAttribute("data-sourcepos") ?? "")?.[1] ?? NaN);
    return { line, offset: Math.round(box.top - top) };
  }
  return null;
};

/** The `n`th workspace `?many` adds: closed, with no folders. */
function notebook(n: number): FakeWorkspace {
  return {
    id: `n${String(n)}`,
    name: `Notebook ${String(n)}`,
    roots: [],
    open: false,
    theme: null,
  };
}

const params = new URLSearchParams(location.search);
const open = params.get("open");
const garden = params.get("garden");
const many = Number(params.get("many") ?? "0");
const blank = params.has("blank");
const recent = Object.entries(__LX_FIXTURES__.docs)
  .slice(0, Number(params.get("recent") ?? "0"))
  .map(([path, doc]) => ({ path, title: doc.title, openedMs: 0 }));
const options: FakeOptions = {
  persist: true,
  workspaces: [
    { ...studio(__LX_FIXTURES__), open: !blank },
    ...(garden === null ? [] : [{ ...GARDEN, open: garden === "open" }]),
    ...Array.from({ length: many }, (_, i) => notebook(i + 1)),
  ],
  ...(blank ? { current: null } : {}),
  ...(recent.length > 0 ? { recent } : {}),
  ...(open === null ? {} : { initial: open }),
};
const backend = new FakeBackend(__LX_FIXTURES__, options);
window.__fake = backend;
const app = new App(backend, byId("lx-app"));
window.__lxPage = () => {
  const page = app.view.page;
  return page && { ready: page.ready, text: page.text, fraction: page.fraction, dark: page.dark };
};
await app.start();
document.documentElement.dataset.lxReady = "";
