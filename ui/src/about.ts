// About Lectern (the ⋯ menu): the version, the licences and the project's page. Loaded on first
// use.
import { h } from "./dom";

export interface AboutHost {
  version(): string;
  portable(): boolean;
  /** Opens a web page in the browser. */
  openLink(url: string): void;
}

const PROJECT_URL = "https://github.com/tacticiankerala/lectern";
/** The bundled fonts, each under the SIL Open Font License 1.1 (fonts/LICENSES). */
const FONTS = [
  "Inter",
  "Atkinson Hyperlegible Next",
  "Literata",
  "Source Serif 4",
  "JetBrains Mono",
];
/** The app icon (src-tauri/icons/source.svg, at a 64-unit scale): a book on a lectern. */
const ICON =
  '<svg viewBox="0 0 64 64" width="56" height="56" aria-hidden="true">' +
  '<defs><linearGradient id="lx-about-plate" x1="0" y1="0" x2="0" y2="1">' +
  '<stop offset="0" stop-color="#2e231b"/><stop offset="1" stop-color="#1b140f"/>' +
  "</linearGradient></defs>" +
  '<rect width="64" height="64" rx="14" fill="url(#lx-about-plate)"/>' +
  '<path d="M8 12Q24 11 32 20V40Q24 35 8 36Z" fill="#f4e8d2"/>' +
  '<path d="M56 12Q40 11 32 20V40Q40 35 56 36Z" fill="#dcc29c"/>' +
  '<g fill="#9a5b2e"><rect x="6" y="40" width="52" height="4" rx="1"/>' +
  '<rect x="28" y="44" width="8" height="8"/><rect x="16" y="52" width="32" height="4" rx="2"/></g>' +
  "</svg>";

export class About {
  private readonly backdrop: HTMLElement;
  private readonly dialog: HTMLElement;
  private readonly version: HTMLElement;
  private returnFocus: Element | null = null;

  constructor(
    root: HTMLElement,
    private readonly host: AboutHost,
  ) {
    const close = h(
      "button",
      { type: "button", class: "icon-btn about-close", "aria-label": "Close" },
      "×",
    );
    close.addEventListener("click", () => {
      this.close();
    });
    const link = h(
      "button",
      { type: "button", class: "about-link" },
      PROJECT_URL.replace("https://", ""),
    );
    link.addEventListener("click", () => {
      this.host.openLink(PROJECT_URL);
    });
    const mark = h("div", { class: "about-mark" });
    mark.innerHTML = ICON;
    this.version = h("p", { class: "about-version" });
    this.dialog = h(
      "div",
      { class: "about", role: "dialog", "aria-modal": "true", "aria-labelledby": "lx-about-title" },
      close,
      mark,
      h("h2", { id: "lx-about-title" }, "Lectern"),
      this.version,
      h("p", {}, "A fast, native Markdown reader for Windows."),
      link,
      h("p", { class: "about-legal" }, "© 2026 Sreenath Nannatt. MIT licence."),
      h(
        "p",
        { class: "about-fonts" },
        `Fonts: ${FONTS.slice(0, -1).join(", ")} and ${FONTS[FONTS.length - 1] ?? ""}, each under the SIL Open Font License 1.1.`,
      ),
    );
    this.dialog.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        this.close();
      }
    });
    this.backdrop = h("div", { class: "prefs-backdrop about-backdrop" }, this.dialog);
    this.backdrop.hidden = true;
    this.backdrop.addEventListener("pointerdown", (e) => {
      if (e.target === this.backdrop) this.close();
    });
    root.append(this.backdrop);
  }

  get isOpen(): boolean {
    return !this.backdrop.hidden;
  }

  open(): void {
    if (this.isOpen) {
      return;
    }
    this.returnFocus = document.activeElement;
    const portable = this.host.portable() ? " · portable" : "";
    this.version.textContent = `Version ${this.host.version()}${portable}`;
    this.backdrop.hidden = false;
    this.dialog.querySelector<HTMLElement>(".about-close")?.focus();
  }

  close(): void {
    if (!this.isOpen) {
      return;
    }
    this.backdrop.hidden = true;
    if (this.returnFocus instanceof HTMLElement && this.returnFocus.isConnected) {
      this.returnFocus.focus({ preventScroll: true });
    }
  }
}
