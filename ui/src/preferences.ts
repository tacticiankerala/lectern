// Preferences (Ctrl+, or the ⋯ menu, spec §6): the library's folders (add, remove, retry), path
// mappings (from/to rows, saved as a field loses focus), the editor command, automatic update
// checks and the version. Loaded on first use.
import { h } from "./dom";
import type { EditorPref } from "./generated/EditorPref";
import type { LibraryPayload } from "./generated/LibraryPayload";
import type { PathMapping } from "./generated/PathMapping";
import type { Settings } from "./generated/Settings";
import type { SettingsPatch } from "./generated/SettingsPatch";

export interface PreferencesHost {
  settings(): Settings;
  library(): LibraryPayload;
  version(): string;
  portable(): boolean;
  update(patch: SettingsPatch): void;
  addFolder(): void;
  removeRoot(path: string): void;
  retryRoot(path: string): void;
}

interface MappingRow {
  el: HTMLElement;
  from: HTMLInputElement;
  to: HTMLInputElement;
}

export class Preferences {
  private readonly backdrop: HTMLElement;
  private readonly dialog: HTMLElement;
  private readonly roots: HTMLElement;
  private readonly mappingList: HTMLElement;
  private mappings: MappingRow[] = [];
  private readonly auto: HTMLInputElement;
  private readonly custom: HTMLInputElement;
  private readonly command: HTMLInputElement;
  private readonly autoUpdate: HTMLInputElement;
  private readonly badges: HTMLInputElement;
  private readonly version: HTMLElement;
  private returnFocus: Element | null = null;

  constructor(
    root: HTMLElement,
    private readonly host: PreferencesHost,
  ) {
    const close = h(
      "button",
      { type: "button", class: "icon-btn prefs-close", "aria-label": "Close" },
      "×",
    );
    close.addEventListener("click", () => {
      this.close();
    });

    this.roots = h("ul", { class: "prefs-roots" });
    const addFolder = h("button", { type: "button", class: "btn" }, "Add folder…");
    addFolder.addEventListener("click", () => {
      this.host.addFolder();
    });

    this.mappingList = h("div", { class: "prefs-mappings" });
    const addMapping = h("button", { type: "button", class: "btn" }, "Add mapping");
    addMapping.addEventListener("click", () => {
      this.addMappingRow({ from: "", to: "" }).from.focus();
    });

    this.auto = h("input", { type: "radio", name: "lx-editor", value: "auto" });
    this.custom = h("input", { type: "radio", name: "lx-editor", value: "custom" });
    this.command = h("input", {
      type: "text",
      class: "prefs-command",
      "aria-label": "Editor command",
      placeholder: 'code --goto "{path}:{line}"',
      spellcheck: "false",
      autocomplete: "off",
    });
    for (const input of [this.auto, this.custom]) {
      input.addEventListener("change", () => {
        if (this.custom.checked) this.command.focus();
        this.saveEditor();
      });
    }
    this.command.addEventListener("change", () => {
      this.custom.checked = true;
      this.saveEditor();
    });

    this.badges = h("input", { type: "checkbox", name: "lx-status-badges" });
    this.badges.addEventListener("change", () => {
      this.host.update({ showStatusBadges: this.badges.checked });
    });
    this.autoUpdate = h("input", { type: "checkbox", name: "lx-auto-update" });
    this.autoUpdate.addEventListener("change", () => {
      this.host.update({ autoUpdate: this.autoUpdate.checked });
    });
    this.version = h("p", { class: "prefs-version" });

    this.dialog = h(
      "div",
      { class: "prefs", role: "dialog", "aria-modal": "true", "aria-label": "Preferences" },
      h("div", { class: "prefs-head" }, h("h2", {}, "Preferences"), close),
      section(
        "Libraries",
        "Folders shown in the sidebar.",
        this.roots,
        addFolder,
        h("label", { class: "prefs-check prefs-badges" }, this.badges, "Show folder status badges"),
        h(
          "p",
          { class: "prefs-hint" },
          "Shows a badge on folders whose README.md has a ",
          h("code", {}, "status:"),
          " field in its frontmatter, e.g. ",
          h("code", {}, "status: active"),
          ". Green for active, red for blocked, grey for parked, blue for done; other values appear neutral.",
        ),
      ),
      section(
        "Path mappings",
        "Absolute paths in notes that start with From open from To instead, such as /home/me/shared → S:\\Shared.",
        this.mappingList,
        addMapping,
      ),
      section(
        "Editor",
        "Open in editor (Ctrl+E) uses VS Code when it is installed, else the file's default app.",
        h("label", { class: "prefs-check" }, this.auto, "Automatic"),
        h("label", { class: "prefs-check" }, this.custom, "Custom command"),
        this.command,
        h("p", { class: "prefs-hint" }, "{path} and {line} stand for the file and the line."),
      ),
      section(
        "Updates",
        null,
        h("label", { class: "prefs-check" }, this.autoUpdate, "Check for updates automatically"),
      ),
      this.version,
    );
    this.dialog.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        this.close();
      }
    });
    this.backdrop = h("div", { class: "prefs-backdrop" }, this.dialog);
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
    const s = this.host.settings();
    this.mappingList.replaceChildren();
    this.mappings = [];
    for (const mapping of s.pathMappings) {
      this.addMappingRow(mapping);
    }
    this.auto.checked = s.editor.mode === "auto";
    this.custom.checked = s.editor.mode === "custom";
    this.command.value = s.editor.mode === "custom" ? s.editor.command : "";
    this.autoUpdate.checked = s.autoUpdate;
    this.badges.checked = s.showStatusBadges;
    const portable = this.host.portable() ? " · portable" : "";
    this.version.textContent = `Lectern ${this.host.version()}${portable}`;
    this.refresh();
    this.backdrop.hidden = false;
    this.dialog.querySelector<HTMLElement>(".prefs-close")?.focus();
  }

  /** Saves what is being typed, then hides the dialog. */
  close(): void {
    if (!this.isOpen) {
      return;
    }
    this.saveMappings();
    this.backdrop.hidden = true;
    if (this.returnFocus instanceof HTMLElement && this.returnFocus.isConnected) {
      this.returnFocus.focus({ preventScroll: true });
    }
  }

  /** Lists the library's roots again, as they changed. */
  refresh(): void {
    const roots = this.host.library().roots;
    if (roots.length === 0) {
      this.roots.replaceChildren(h("li", { class: "prefs-empty" }, "No folders yet."));
      return;
    }
    this.roots.replaceChildren(
      ...roots.map((root) => {
        const actions = h("span", { class: "prefs-root-actions" });
        let state = "";
        if (root.state.state === "unavailable") {
          state = root.state.reason;
          const retry = h("button", { type: "button", class: "btn" }, "Retry");
          retry.addEventListener("click", () => {
            this.host.retryRoot(root.path);
          });
          actions.append(retry);
        } else if (root.state.state === "scanning") {
          state = "Scanning…";
        }
        const remove = h(
          "button",
          { type: "button", class: "btn", "aria-label": `Remove ${root.name}` },
          "Remove",
        );
        remove.addEventListener("click", () => {
          this.host.removeRoot(root.path);
        });
        actions.append(remove);
        return h(
          "li",
          { class: "prefs-root" },
          h(
            "span",
            { class: "prefs-root-text" },
            h("span", { class: "prefs-root-name" }, root.name),
            h("span", { class: "prefs-root-path" }, root.path),
            ...(state === "" ? [] : [h("span", { class: "prefs-root-state" }, state)]),
          ),
          actions,
        );
      }),
    );
  }

  private addMappingRow(mapping: PathMapping): MappingRow {
    const from = h("input", {
      type: "text",
      "aria-label": "Map from",
      placeholder: "/home/me/shared",
      spellcheck: "false",
      autocomplete: "off",
    });
    const to = h("input", {
      type: "text",
      "aria-label": "Map to",
      placeholder: "S:\\Shared",
      spellcheck: "false",
      autocomplete: "off",
    });
    from.value = mapping.from;
    to.value = mapping.to;
    const remove = h(
      "button",
      {
        type: "button",
        class: "icon-btn",
        "aria-label": "Remove mapping",
        title: "Remove mapping",
      },
      "×",
    );
    const row: MappingRow = {
      el: h(
        "div",
        { class: "prefs-mapping" },
        from,
        h("span", { "aria-hidden": "true" }, "→"),
        to,
        remove,
      ),
      from,
      to,
    };
    for (const input of [from, to]) {
      input.addEventListener("blur", () => {
        this.saveMappings();
      });
    }
    remove.addEventListener("click", () => {
      row.el.remove();
      this.mappings = this.mappings.filter((m) => m !== row);
      this.saveMappings();
    });
    this.mappings.push(row);
    this.mappingList.append(row.el);
    return row;
  }

  /** Saves the complete rows, when they differ from the settings; half-filled rows wait. */
  private saveMappings(): void {
    const mappings: PathMapping[] = this.mappings
      .map((m) => ({ from: m.from.value.trim(), to: m.to.value.trim() }))
      .filter((m) => m.from !== "" && m.to !== "");
    if (JSON.stringify(mappings) !== JSON.stringify(this.host.settings().pathMappings)) {
      this.host.update({ pathMappings: mappings });
    }
  }

  /** Custom needs a command; until it has one, Automatic stays in effect. */
  private saveEditor(): void {
    const command = this.command.value.trim();
    const editor: EditorPref =
      this.custom.checked && command !== "" ? { mode: "custom", command } : { mode: "auto" };
    if (JSON.stringify(editor) !== JSON.stringify(this.host.settings().editor)) {
      this.host.update({ editor });
    }
  }
}

function section(title: string, lede: string | null, ...content: HTMLElement[]): HTMLElement {
  return h(
    "section",
    { class: "prefs-section" },
    h("h3", {}, title),
    ...(lede === null ? [] : [h("p", { class: "prefs-lede" }, lede)]),
    ...content,
  );
}
