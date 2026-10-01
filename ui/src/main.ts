// Lectern's UI on Tauri.
import { App } from "./app";
import { TauriBackend } from "./backend-tauri";
import { byId } from "./dom";

const app = new App(new TauriBackend(), byId("lx-app"));
await app.start();
