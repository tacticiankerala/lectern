// Temporary debug harness until Task 9 builds the UI: shows the startup document (or its error)
// unstyled, and re-renders on open-request and doc-changed.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DocChanged } from "./generated/DocChanged";
import type { OpenRequest } from "./generated/OpenRequest";
import type { OpenResult } from "./generated/OpenResult";
import type { StartupPayload } from "./generated/StartupPayload";

function nextPaint(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        resolve();
      });
    });
  });
}

function show(result: OpenResult | null): void {
  const heading = document.createElement("h1");
  const body = document.createElement("div");
  if (result === null) {
    heading.textContent = "Lectern";
  } else if (result.status === "ok") {
    heading.textContent = result.doc.title;
    body.innerHTML = result.doc.html;
  } else {
    heading.textContent = result.error.message;
  }
  document.body.replaceChildren(heading, body);
}

async function open(path: string, t0Ms: number | null): Promise<void> {
  show(await invoke<OpenResult>("open_document", { path }));
  if (t0Ms !== null) {
    await nextPaint();
    void invoke("perf_mark", { name: "warm-open", ms: Date.now() - t0Ms });
  }
}

async function main(): Promise<void> {
  // Listen before startup: once startup has run, second launches arrive only as events.
  await listen<OpenRequest>("open-request", (e) => void open(e.payload.path, e.payload.t0Ms));
  await listen<DocChanged>("doc-changed", (e) => void open(e.payload.path, null));
  const payload = await invoke<StartupPayload>("startup");
  // Kept for inspection from the devtools.
  (window as unknown as { lxStartup: StartupPayload }).lxStartup = payload;
  show(payload.initial);
  await nextPaint();
  void invoke("perf_mark", { name: "first-paint" });
  void invoke("show_window");
}

void main();
