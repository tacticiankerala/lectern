// The UI on the fake backend, for Playwright: `/?open=<path>` stands in for a launch argument.
// `?garden` adds a second workspace, Garden, closed (`?garden=open`: open in another window),
// `?many=<n>` adds n more, "Notebook 1" to "Notebook n", closed with no folders, and `?blank`
// starts a blank window. `window.__lxDocText()` reads the page's visible text as the review
// comments do, to check it against core's (`__fake.coreText`). `window.__lxPage()` is what the HTML
// page on screen has reported through its bridge.
import { App } from "../src/app";
import { docText } from "../src/comments-text";
import { byId } from "../src/dom";
import type { PageFrame } from "../src/page-frame";
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
    __lxDocText: () => string;
    __lxPage: () => PageState | null;
  }
}

window.__lxDocText = () => {
  const doc = document.querySelector(".doc");
  return doc ? docText(doc) : "";
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
const options: FakeOptions = {
  persist: true,
  workspaces: [
    { ...studio(__LX_FIXTURES__), open: !blank },
    ...(garden === null ? [] : [{ ...GARDEN, open: garden === "open" }]),
    ...Array.from({ length: many }, (_, i) => notebook(i + 1)),
  ],
  ...(blank ? { current: null } : {}),
  ...(open === null ? {} : { initial: open }),
};
const backend = new FakeBackend(__LX_FIXTURES__, options);
window.__fake = backend;
const app = new App(backend, byId("lx-app"));
window.__lxPage = () => {
  const page = app.view.page as PageFrame | null;
  return page && { ready: page.ready, text: page.text, fraction: page.fraction, dark: page.dark };
};
await app.start();
document.documentElement.dataset.lxReady = "";
