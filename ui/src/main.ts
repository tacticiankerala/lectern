import { getCurrentWindow } from "@tauri-apps/api/window";

document.body.textContent = "Lectern";

// The window starts hidden; show it once the first frame has painted, so there is no white flash.
requestAnimationFrame(() => {
  requestAnimationFrame(() => {
    void getCurrentWindow().show();
  });
});
