// The UI on the fake backend, for Playwright: `/?open=<path>` stands in for a launch argument.
import { App } from "../src/app";
import { byId } from "../src/dom";
import { FakeBackend, type Fixtures } from "./backend-fake";

/** dev/fixtures.json, inlined by `build.mjs --fake`. */
declare const __LX_FIXTURES__: Fixtures;

const open = new URLSearchParams(location.search).get("open");
const backend = new FakeBackend(__LX_FIXTURES__, open === null ? {} : { initial: open });
window.__fake = backend;
const app = new App(backend, byId("lx-app"));
await app.start();
document.documentElement.dataset.lxReady = "";
