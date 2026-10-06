// The UI on the fake backend, for Playwright: `/?open=<path>` stands in for a launch argument.
// `window.__lxDocText()` reads the page's visible text as the review comments do, to check it
// against core's (`__fake.coreText`).
import { App } from "../src/app";
import { docText } from "../src/comments-text";
import { byId } from "../src/dom";
import { FakeBackend, type Fixtures } from "./backend-fake";

/** dev/fixtures.json, inlined by `build.mjs --fake`. */
declare const __LX_FIXTURES__: Fixtures;

declare global {
  interface Window {
    __lxDocText: () => string;
  }
}

window.__lxDocText = () => {
  const doc = document.querySelector(".doc");
  return doc ? docText(doc) : "";
};

const open = new URLSearchParams(location.search).get("open");
const backend = new FakeBackend(
  __LX_FIXTURES__,
  open === null ? { persist: true } : { initial: open, persist: true },
);
window.__fake = backend;
const app = new App(backend, byId("lx-app"));
await app.start();
document.documentElement.dataset.lxReady = "";
