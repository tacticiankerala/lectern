// What the document area shows without a document: the welcome screen, or why a file didn't open.
import { MARKDOWN_PATH, h } from "./dom";
import type { OpenError } from "./generated/OpenError";
import type { RecentEntry } from "./generated/RecentEntry";

export interface WelcomeActions {
  openFile: () => void;
  addFolder: () => void;
  openRecent: (path: string) => void;
  removeRecent: (path: string) => void;
}

export interface ErrorActions {
  retry: () => void;
  forget: () => void;
  openWithDefaultApp: () => void;
  reveal: () => void;
  /** Looks for a file that isn't there by its name: it may have moved. */
  search: () => void;
}

const SHORTCUTS: [string, string][] = [
  ["Ctrl+O", "Open a file"],
  ["Ctrl+Shift+N", "Add a folder to the library"],
  ["Ctrl+P", "Quick open"],
  ["Ctrl+Shift+.", "Breadcrumb chooser"],
  ["Ctrl+Shift+F", "Search the library"],
  ["Ctrl+F", "Find in page"],
  ["Alt+← / Alt+→", "Back / forward"],
  ["Ctrl+= / Ctrl+- / Ctrl+0", "Text size"],
  ["Ctrl+Alt+= / Ctrl+Alt+- / Ctrl+Alt+0", "Sidebar text size"],
  ["Ctrl+Shift+T", "Light or dark theme"],
  ["Ctrl+B / Ctrl+Shift+O", "Library / outline"],
  ["F11", "Focus mode"],
  ["Ctrl+E", "Open in editor"],
];

const ERROR_TITLES: Record<OpenError["kind"], string> = {
  notFound: "This file isn't there",
  permission: "Lectern can't open this file",
  binary: "This doesn't look like a text file",
  io: "This file couldn't be read",
};

interface ButtonOptions {
  /** The shortcut, shown after the label. */
  keys?: string;
  primary?: boolean;
}

function button(label: string, onClick: () => void, opts: ButtonOptions = {}): HTMLElement {
  const b = h("button", { type: "button", class: opts.primary ? "btn primary" : "btn" }, label);
  if (opts.keys !== undefined) {
    b.append(h("kbd", {}, opts.keys));
  }
  b.addEventListener("click", onClick);
  return b;
}

function heading(text: string): HTMLElement {
  return h("h2", { class: "state-heading" }, text);
}

export function renderWelcome(
  host: HTMLElement,
  recent: RecentEntry[],
  actions: WelcomeActions,
): void {
  const welcome = h(
    "div",
    { class: "welcome" },
    h("h1", { class: "welcome-title" }, "Lectern"),
    h("p", { class: "welcome-lede" }, "Open a Markdown file, or add a folder to your library."),
    h(
      "div",
      { class: "welcome-actions" },
      button("Open file…", actions.openFile, { keys: "Ctrl+O", primary: true }),
      button("Add folder…", actions.addFolder, { keys: "Ctrl+Shift+N" }),
    ),
  );
  if (recent.length > 0) {
    const list = h("ul", { class: "recent-list" });
    for (const entry of recent) {
      const item = h(
        "button",
        { type: "button", class: "recent-item", title: entry.path },
        h("span", { class: "recent-title" }, entry.title),
        h("span", { class: "recent-path" }, entry.path),
      );
      item.addEventListener("click", () => {
        actions.openRecent(entry.path);
      });
      const remove = h(
        "button",
        {
          type: "button",
          class: "recent-remove",
          "aria-label": `Remove ${entry.title} from recent`,
          title: "Remove from recent",
        },
        "×",
      );
      remove.addEventListener("click", () => {
        actions.removeRecent(entry.path);
      });
      list.append(h("li", { class: "recent-row" }, item, remove));
    }
    welcome.append(heading("Recent"), list);
  }
  const keys = h("dl", { class: "keys" });
  for (const [key, action] of SHORTCUTS) {
    keys.append(h("dt", {}, h("kbd", {}, key)), h("dd", {}, action));
  }
  welcome.append(heading("Shortcuts"), keys);
  host.replaceChildren(welcome);
}

export function renderError(host: HTMLElement, error: OpenError, actions: ErrorActions): void {
  const buttons = [button("Retry", actions.retry, { primary: true })];
  if (error.kind === "notFound") {
    buttons.push(button("Search for it", actions.search));
  }
  buttons.push(button("Remove from recent", actions.forget));
  if (error.kind === "binary") {
    // Rust would open a Markdown-named file in the reader again, so that one is revealed instead.
    buttons.push(
      MARKDOWN_PATH.test(error.path)
        ? button("Reveal in Explorer", actions.reveal)
        : button("Open with default app", actions.openWithDefaultApp),
    );
  }
  host.replaceChildren(
    h(
      "div",
      { class: "state state-error", role: "alert" },
      h("h1", { class: "state-title" }, ERROR_TITLES[error.kind]),
      h("p", { class: "state-detail" }, error.message),
      ...(error.message.includes(error.path) ? [] : [h("p", { class: "state-path" }, error.path)]),
      h("div", { class: "state-actions" }, ...buttons),
    ),
  );
}
