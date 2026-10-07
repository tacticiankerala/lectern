// Preferences (Ctrl+, or the ⋯ menu, spec §6), in two parts. This workspace: its name, its
// libraries' folders (add, remove, retry) and its theme, the shared one ("Same as other windows")
// or its own mode and light and dark themes. A blank window has no such part. All windows: folder
// status badges, review comments on or off, path mappings (from/to rows, saved as a field loses
// focus), the editor command, automatic update checks, and the workspaces, to rename or delete (not
// one open in a window). Then the version. Settings changed elsewhere show at once in the switches,
// the theme and the path mappings, unless the mappings were edited here: those are what closing
// saves. Loaded on first use.
import { h } from "./dom";
import type { EditorPref } from "./generated/EditorPref";
import type { LibraryPayload } from "./generated/LibraryPayload";
import type { PathMapping } from "./generated/PathMapping";
import type { Settings } from "./generated/Settings";
import type { SettingsPatch } from "./generated/SettingsPatch";
import type { ThemeId } from "./generated/ThemeId";
import type { ThemeMode } from "./generated/ThemeMode";
import type { WorkspaceSummary } from "./generated/WorkspaceSummary";
import { THEMES } from "./themes";

export interface PreferencesHost {
  settings(): Settings;
  library(): LibraryPayload;
  version(): string;
  portable(): boolean;
  update(patch: SettingsPatch): void;
  addFolder(): void;
  removeRoot(path: string): void;
  retryRoot(path: string): void;
  /** This window's workspace; null for a blank window. */
  workspace(): WorkspaceSummary | null;
  /** Every workspace, in creation order. */
  workspaces(): WorkspaceSummary[];
  /** Renames workspace `id`; false when it couldn't. */
  renameWorkspace(id: string, name: string): Promise<boolean>;
  deleteWorkspace(id: string): void;
  /** Gives this window's workspace a theme of its own, or has it follow the shared one. */
  setOwnTheme(own: boolean): void;
}

const MODES: [ThemeMode, string][] = [
  ["system", "System"],
  ["light", "Light"],
  ["dark", "Dark"],
];

interface MappingRow {
  el: HTMLElement;
  from: HTMLInputElement;
  to: HTMLInputElement;
}

export class Preferences {
  private readonly backdrop: HTMLElement;
  private readonly dialog: HTMLElement;
  /** The This workspace part, hidden in a blank window. */
  private readonly thisWorkspace: HTMLElement;
  private readonly name: HTMLInputElement;
  private readonly roots: HTMLElement;
  private readonly ownTheme: HTMLInputElement;
  private readonly sharedThemeHint: HTMLElement;
  private readonly themePickers: HTMLElement;
  private readonly modes: HTMLInputElement[];
  private readonly lightTheme: HTMLSelectElement;
  private readonly darkTheme: HTMLSelectElement;
  private readonly workspaceList: HTMLElement;
  private readonly mappingList: HTMLElement;
  private mappings: MappingRow[] = [];
  /** The mapping rows were edited here since they last matched the settings. */
  private mappingsEdited = false;
  private readonly auto: HTMLInputElement;
  private readonly custom: HTMLInputElement;
  private readonly command: HTMLInputElement;
  private readonly autoUpdate: HTMLInputElement;
  private readonly badges: HTMLInputElement;
  private readonly reviewComments: HTMLInputElement;
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

    this.name = h("input", {
      type: "text",
      class: "prefs-ws-name",
      "aria-label": "Workspace name",
      spellcheck: "false",
      autocomplete: "off",
      maxlength: "60",
    });
    this.name.addEventListener("change", () => void this.renameCurrent());

    this.roots = h("ul", { class: "prefs-roots" });
    const addFolder = h("button", { type: "button", class: "btn" }, "Add folder…");
    addFolder.addEventListener("click", () => {
      this.host.addFolder();
    });

    this.ownTheme = h("input", { type: "checkbox", name: "lx-own-theme" });
    this.ownTheme.addEventListener("change", () => {
      this.host.setOwnTheme(this.ownTheme.checked);
    });
    this.sharedThemeHint = h("p", { class: "prefs-hint" }, "Same as other windows.");
    this.modes = MODES.map(([mode]) => {
      const input = h("input", { type: "radio", name: "lx-ws-theme-mode", value: mode });
      input.addEventListener("change", () => {
        this.host.update({ themeMode: mode });
      });
      return input;
    });
    const segmented = h("div", { class: "segmented", role: "radiogroup", "aria-label": "Theme" });
    MODES.forEach(([, label], i) => {
      segmented.append(h("label", { class: "segment" }, this.modes[i] ?? "", h("span", {}, label)));
    });
    this.lightTheme = themeSelect("light", "Light theme");
    this.darkTheme = themeSelect("dark", "Dark theme");
    this.lightTheme.addEventListener("change", () => {
      this.host.update({ lightTheme: this.lightTheme.value as ThemeId });
    });
    this.darkTheme.addEventListener("change", () => {
      this.host.update({ darkTheme: this.darkTheme.value as ThemeId });
    });
    this.themePickers = h(
      "div",
      { class: "prefs-theme" },
      segmented,
      h("label", { class: "prefs-theme-row" }, "Light", select(this.lightTheme)),
      h("label", { class: "prefs-theme-row" }, "Dark", select(this.darkTheme)),
    );
    this.workspaceList = h("ul", { class: "prefs-workspaces" });

    this.mappingList = h("div", { class: "prefs-mappings" });
    const addMapping = h("button", { type: "button", class: "btn" }, "Add mapping");
    addMapping.addEventListener("click", () => {
      this.mappingsEdited = true;
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
    this.reviewComments = h("input", { type: "checkbox", name: "lx-review-comments" });
    this.reviewComments.addEventListener("change", () => {
      this.host.update({ reviewComments: this.reviewComments.checked });
    });
    this.autoUpdate = h("input", { type: "checkbox", name: "lx-auto-update" });
    this.autoUpdate.addEventListener("change", () => {
      this.host.update({ autoUpdate: this.autoUpdate.checked });
    });
    this.version = h("p", { class: "prefs-version" });

    this.thisWorkspace = h(
      "div",
      { class: "prefs-group" },
      h("h3", { class: "prefs-group-title" }, "This workspace"),
      section("Name", null, this.name),
      section("Libraries", "Folders shown in the sidebar.", this.roots, addFolder),
      section(
        "Theme",
        null,
        h("label", { class: "prefs-check" }, this.ownTheme, "Use its own theme"),
        this.sharedThemeHint,
        this.themePickers,
      ),
    );
    this.dialog = h(
      "div",
      { class: "prefs", role: "dialog", "aria-modal": "true", "aria-label": "Preferences" },
      h("div", { class: "prefs-head" }, h("h2", {}, "Preferences"), close),
      this.thisWorkspace,
      h("h3", { class: "prefs-group-title" }, "All windows"),
      section(
        "Status badges",
        null,
        h("label", { class: "prefs-check" }, this.badges, "Show folder status badges"),
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
        "Reading",
        null,
        h("label", { class: "prefs-check" }, this.reviewComments, "Review comments"),
        h(
          "p",
          { class: "prefs-hint" },
          "Comments are saved next to each note as <note>.review.md, a Markdown file Claude, Codex or another AI agent can read and reply in.",
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
      section(
        "Workspaces",
        "Each window shows one. Deleting a workspace forgets it, never its files.",
        this.workspaceList,
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
    this.renderMappings(s.pathMappings);
    this.auto.checked = s.editor.mode === "auto";
    this.custom.checked = s.editor.mode === "custom";
    this.command.value = s.editor.mode === "custom" ? s.editor.command : "";
    const portable = this.host.portable() ? " · portable" : "";
    this.version.textContent = `Lectern ${this.host.version()}${portable}`;
    this.name.value = this.host.workspace()?.name ?? "";
    this.syncSettings();
    this.syncWorkspaces();
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

  /**
   * Brings the switches and the theme in line with the settings, which may have changed elsewhere.
   * Fields being typed in are left alone.
   */
  syncSettings(): void {
    const s = this.host.settings();
    this.autoUpdate.checked = s.autoUpdate;
    this.badges.checked = s.showStatusBadges;
    this.reviewComments.checked = s.reviewComments;
    const own = this.host.workspace()?.ownTheme ?? false;
    this.ownTheme.checked = own;
    this.sharedThemeHint.hidden = own;
    this.themePickers.hidden = !own;
    for (const input of this.modes) {
      input.checked = input.value === s.themeMode;
    }
    this.lightTheme.value = s.lightTheme;
    this.darkTheme.value = s.darkTheme;
    // Mappings changed elsewhere replace rows that weren't edited here.
    if (
      !this.mappingsEdited &&
      JSON.stringify(this.rowValues()) !== JSON.stringify(s.pathMappings)
    ) {
      this.renderMappings(s.pathMappings);
    }
  }

  /** The workspaces changed: this one's name and the list follow. */
  syncWorkspaces(): void {
    const current = this.host.workspace();
    this.thisWorkspace.hidden = current === null;
    if (current && document.activeElement !== this.name) {
      this.name.value = current.name;
    }
    this.syncSettings();
    // A rename being typed in the list stays.
    const typing = document.activeElement;
    if (typing instanceof HTMLInputElement && this.workspaceList.contains(typing)) {
      return;
    }
    const list = this.host.workspaces();
    this.workspaceList.replaceChildren(
      ...list.map((ws) => this.workspaceRow(ws, list.length === 1)),
    );
  }

  /**
   * A workspace in the list: its name and where it's open, with Rename and Delete. Delete is off
   * for the only workspace (`only`) and for one open in a window, saying why.
   */
  private workspaceRow(ws: WorkspaceSummary, only: boolean): HTMLElement {
    const where = ws.current ? "This window" : ws.open ? "Open in another window" : "";
    const text = h(
      "span",
      { class: "prefs-root-text" },
      h("span", { class: "prefs-root-name" }, ws.name),
      ...(where === "" ? [] : [h("span", { class: "prefs-root-path" }, where)]),
    );
    const rename = h(
      "button",
      { type: "button", class: "btn", "aria-label": `Rename ${ws.name}` },
      "Rename",
    );
    const remove = h(
      "button",
      { type: "button", class: "btn", "aria-label": `Delete ${ws.name}` },
      "Delete",
    );
    if (only || ws.open) {
      remove.disabled = true;
      remove.title = only ? "Lectern needs at least one workspace." : "Close its window first.";
    }
    const row = h(
      "li",
      { class: "prefs-root", "data-id": ws.id },
      text,
      h("span", { class: "prefs-root-actions" }, rename, remove),
    );
    rename.addEventListener("click", () => {
      this.renameInList(ws, text);
    });
    remove.addEventListener("click", () => {
      this.host.deleteWorkspace(ws.id);
    });
    return row;
  }

  /** Swaps a listed workspace's name for a field: Enter renames it, Esc cancels. */
  private renameInList(ws: WorkspaceSummary, text: HTMLElement): void {
    const input = h("input", {
      type: "text",
      "aria-label": "Workspace name",
      spellcheck: "false",
      autocomplete: "off",
      maxlength: "60",
    });
    input.value = ws.name;
    const done = (): void => {
      // The field goes first, so the list may be drawn again; the focus goes back to Rename.
      input.blur();
      this.syncWorkspaces();
      [...this.workspaceList.querySelectorAll<HTMLElement>("li")]
        .find((li) => li.dataset.id === ws.id)
        ?.querySelector<HTMLElement>("button")
        ?.focus();
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        void this.host.renameWorkspace(ws.id, input.value).then((renamed) => {
          if (renamed) done();
        });
      } else if (e.key === "Escape") {
        // Before the dialog's Esc, which would close it.
        e.preventDefault();
        e.stopPropagation();
        done();
      }
    });
    text.replaceChildren(input);
    input.focus();
    input.select();
  }

  /** Renames this window's workspace as its name field says; refused, the name comes back. */
  private async renameCurrent(): Promise<void> {
    const current = this.host.workspace();
    if (!current || this.name.value === current.name) {
      return;
    }
    if (!(await this.host.renameWorkspace(current.id, this.name.value))) {
      this.name.value = this.host.workspace()?.name ?? "";
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

  /** The mapping rows for `mappings`, none of them edited. */
  private renderMappings(mappings: PathMapping[]): void {
    this.mappingList.replaceChildren();
    this.mappings = [];
    this.mappingsEdited = false;
    for (const mapping of mappings) {
      this.addMappingRow(mapping);
    }
  }

  /** What the rows say, trimmed, half-filled ones included. */
  private rowValues(): PathMapping[] {
    return this.mappings.map((m) => ({ from: m.from.value.trim(), to: m.to.value.trim() }));
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
      input.addEventListener("input", () => {
        this.mappingsEdited = true;
      });
      input.addEventListener("blur", () => {
        this.saveMappings();
      });
    }
    remove.addEventListener("click", () => {
      this.mappingsEdited = true;
      row.el.remove();
      this.mappings = this.mappings.filter((m) => m !== row);
      this.saveMappings();
    });
    this.mappings.push(row);
    this.mappingList.append(row.el);
    return row;
  }

  /**
   * Saves the complete rows, when they were edited here and differ from the settings; half-filled
   * rows wait. Rows that match the settings once more follow changes made elsewhere again.
   */
  private saveMappings(): void {
    if (!this.mappingsEdited) {
      return;
    }
    const rows = this.rowValues();
    const mappings = rows.filter((m) => m.from !== "" && m.to !== "");
    if (JSON.stringify(mappings) !== JSON.stringify(this.host.settings().pathMappings)) {
      this.host.update({ pathMappings: mappings });
    }
    this.mappingsEdited = mappings.length !== rows.length;
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

/** A select of a mode's themes. */
function themeSelect(mode: "light" | "dark", label: string): HTMLSelectElement {
  const el = h("select", { name: `lx-ws-${mode}-theme`, "aria-label": label });
  el.append(
    ...THEMES.filter((t) => t.mode === mode).map((t) => h("option", { value: t.id }, t.name)),
  );
  return el;
}

/** A select in its frame, which draws the arrow. */
function select(el: HTMLSelectElement): HTMLElement {
  return h("span", { class: "select" }, el);
}

function section(title: string, lede: string | null, ...content: HTMLElement[]): HTMLElement {
  return h(
    "section",
    { class: "prefs-section" },
    h("h4", {}, title),
    ...(lede === null ? [] : [h("p", { class: "prefs-lede" }, lede)]),
    ...content,
  );
}
