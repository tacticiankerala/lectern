// The reading panel: the "Aa" popover with the theme mode, the light and dark themes, the fonts,
// size, line height, width and code wrap. Every change goes through the host, which applies it at
// once and saves it; sliders ask for the save to wait until the drag settles. The panel is built
// the first time it opens, and asks for the installed fonts only when a font select opens.
import { h } from "./dom";
import type { Settings } from "./generated/Settings";
import type { SettingsPatch } from "./generated/SettingsPatch";
import type { ThemeMode } from "./generated/ThemeMode";
import { FONT_SIZE, LINE_HEIGHT, MEASURE, THEMES, isDark, type ThemeDef } from "./themes";

export interface ReadingHost {
  settings(): Settings;
  systemDark(): boolean;
  /** Applies a change at once and saves it, after a pause when `debounce` is set. */
  update(patch: SettingsPatch, opts?: { debounce?: boolean }): void;
  listSystemFonts(): Promise<string[]>;
}

const BODY_FONTS = [
  "Segoe UI Variable Text",
  "Inter",
  "Atkinson Hyperlegible Next",
  "Literata",
  "Source Serif 4",
  "Georgia",
  "Sitka Text",
  "Cambria",
];
const CODE_FONTS = ["JetBrains Mono", "Consolas"];
/** Offered for code, after JetBrains Mono, only when it is installed. */
const CASCADIA = "Cascadia Code";
/** The value of each font select's "Other installed font…" option. */
const OTHER = "";
const MODES: [ThemeMode, string][] = [
  ["system", "System"],
  ["light", "Light"],
  ["dark", "Dark"],
];
const FONT_LIST_ID = "lx-rp-fonts";

type FontKind = "body" | "code";

interface FontControl {
  kind: FontKind;
  select: HTMLSelectElement;
  other: HTMLInputElement;
}

interface SwatchGroup {
  mode: "light" | "dark";
  themes: { theme: ThemeDef; input: HTMLInputElement }[];
  caption: HTMLElement;
  /** The tiles and caption; marked `showing` while its mode is the one in effect. */
  block: HTMLElement;
}

/** The built panel's controls. */
interface Controls {
  el: HTMLElement;
  modes: HTMLInputElement[];
  swatches: SwatchGroup[];
  fonts: FontControl[];
  size: HTMLInputElement;
  lineHeight: HTMLInputElement;
  measure: HTMLInputElement;
  fullWidth: HTMLInputElement;
  codeWrap: HTMLInputElement;
  values: Map<HTMLInputElement, HTMLOutputElement>;
}

export class ReadingPanel {
  private ui: Controls | null = null;
  /** The installed fonts, asked for once. */
  private systemFonts: Promise<string[]> | null = null;
  /** The width to go back to when Full width is turned off. */
  private lastMeasure: number = MEASURE.default;

  constructor(
    private readonly button: HTMLElement,
    private readonly root: HTMLElement,
    private readonly host: ReadingHost,
  ) {
    button.addEventListener("click", () => {
      this.toggle();
    });
  }

  get isOpen(): boolean {
    return this.ui !== null && !this.ui.el.hidden;
  }

  toggle(): void {
    if (this.isOpen) {
      this.close();
    } else {
      this.open();
    }
  }

  open(): void {
    const ui = (this.ui ??= this.build());
    this.refresh();
    ui.el.hidden = false;
    this.button.setAttribute("aria-expanded", "true");
    document.addEventListener("pointerdown", this.onOutside);
    (ui.modes.find((input) => input.checked) ?? ui.modes[0])?.focus();
  }

  /** Hides the panel; focus inside it goes back to the button. */
  close(): void {
    const el = this.ui?.el;
    if (!el || el.hidden) {
      return;
    }
    const hadFocus = el.contains(document.activeElement);
    el.hidden = true;
    this.button.setAttribute("aria-expanded", "false");
    document.removeEventListener("pointerdown", this.onOutside);
    if (hadFocus) {
      this.button.focus();
    }
  }

  /** Brings the controls in line with the settings, which may have changed elsewhere. */
  refresh(): void {
    const ui = this.ui;
    if (!ui) {
      return;
    }
    const s = this.host.settings();
    const dark = isDark(s, this.host.systemDark());
    for (const input of ui.modes) {
      input.checked = input.value === s.themeMode;
    }
    for (const group of ui.swatches) {
      group.block.classList.toggle("showing", dark === (group.mode === "dark"));
      const current = group.mode === "light" ? s.lightTheme : s.darkTheme;
      for (const { input } of group.themes) {
        input.checked = input.value === current;
      }
      group.caption.textContent = THEMES.find((t) => t.id === current)?.name ?? "";
    }
    for (const font of ui.fonts) {
      syncFont(font, font.kind === "body" ? s.bodyFont : s.codeFont);
    }
    if (s.measure !== "full") {
      this.lastMeasure = s.measure;
    }
    ui.size.value = String(s.fontSize);
    ui.lineHeight.value = String(s.lineHeight);
    ui.measure.value = String(this.lastMeasure);
    ui.measure.disabled = s.measure === "full";
    ui.fullWidth.checked = s.measure === "full";
    ui.codeWrap.checked = s.codeWrap;
    for (const [input, output] of ui.values) {
      output.textContent = valueText(input);
    }
  }

  private readonly onOutside = (e: Event): void => {
    const target = e.target instanceof Node ? e.target : null;
    if (target && !this.ui?.el.contains(target) && !this.button.contains(target)) {
      this.close();
    }
  };

  private build(): Controls {
    const values = new Map<HTMLInputElement, HTMLOutputElement>();
    const slider = (name: string, label: string, min: number, max: number, step: number) => {
      const input = h("input", {
        type: "range",
        name,
        min: String(min),
        max: String(max),
        step: String(step),
        "aria-label": label,
      });
      const output = h("output", { class: "rp-value" });
      values.set(input, output);
      input.addEventListener("input", () => {
        output.textContent = valueText(input);
      });
      return input;
    };
    const checkbox = (name: string) => h("input", { type: "checkbox", name });

    const modes = MODES.map(([mode]) => {
      const input = h("input", { type: "radio", name: "lx-theme-mode", value: mode });
      input.addEventListener("change", () => {
        this.host.update({ themeMode: mode });
      });
      return input;
    });
    const swatches = (["light", "dark"] as const).map((mode) => this.themeSwatches(mode));
    const fonts = [
      this.fontControl("body", "Text font", BODY_FONTS),
      this.fontControl("code", "Code font", CODE_FONTS),
    ];
    const ui: Controls = {
      el: h("div", { class: "reading-panel", role: "dialog", "aria-label": "Reading settings" }),
      modes,
      swatches,
      fonts,
      size: slider("lx-font-size", "Text size", FONT_SIZE.min, FONT_SIZE.max, 1),
      lineHeight: slider(
        "lx-line-height",
        "Line height",
        LINE_HEIGHT.min,
        LINE_HEIGHT.max,
        LINE_HEIGHT.step,
      ),
      measure: slider("lx-measure", "Text width", MEASURE.min, MEASURE.max, 1),
      fullWidth: checkbox("lx-full-width"),
      codeWrap: checkbox("lx-code-wrap"),
      values,
    };

    const segmented = h("div", { class: "segmented", role: "radiogroup", "aria-label": "Theme" });
    MODES.forEach(([, label], i) => {
      segmented.append(h("label", { class: "segment" }, modes[i] ?? "", h("span", {}, label)));
    });
    const sliderRow = (label: string, input: HTMLInputElement) =>
      row(label, h("div", { class: "rp-slider" }, input, values.get(input) ?? ""));
    const fontRow = (label: string, font: FontControl) =>
      row(
        label,
        h("div", { class: "rp-font" }, h("span", { class: "select" }, font.select), font.other),
      );
    ui.el.hidden = true;
    ui.el.append(
      row("Theme", segmented),
      ...swatches.map((group) =>
        row(group.mode === "light" ? "Light" : "Dark", group.block, "top"),
      ),
      h("div", { class: "rp-rule" }),
      ...fonts.map((font) => fontRow(font.kind === "body" ? "Text" : "Code", font)),
      sliderRow("Size", ui.size),
      sliderRow("Line height", ui.lineHeight),
      sliderRow("Width", ui.measure),
      row("", h("label", { class: "rp-check" }, ui.fullWidth, "Full width"), "check"),
      row("", h("label", { class: "rp-check" }, ui.codeWrap, "Wrap long code lines"), "check"),
      h("datalist", { id: FONT_LIST_ID }),
    );

    ui.size.addEventListener("input", () => {
      this.host.update({ fontSize: Number(ui.size.value) }, { debounce: true });
    });
    ui.lineHeight.addEventListener("input", () => {
      const lineHeight = Math.round(Number(ui.lineHeight.value) * 100) / 100;
      this.host.update({ lineHeight }, { debounce: true });
    });
    ui.measure.addEventListener("input", () => {
      this.lastMeasure = Number(ui.measure.value);
      this.host.update({ measure: this.lastMeasure }, { debounce: true });
    });
    ui.fullWidth.addEventListener("change", () => {
      this.host.update({ measure: ui.fullWidth.checked ? "full" : this.lastMeasure });
    });
    ui.codeWrap.addEventListener("change", () => {
      this.host.update({ codeWrap: ui.codeWrap.checked });
    });
    this.root.append(ui.el);
    return ui;
  }

  /** A radio for each theme of a mode, picking it on a click. */
  private themeSwatches(mode: "light" | "dark"): SwatchGroup {
    const themes = THEMES.filter((t) => t.mode === mode).map((theme) => {
      const input = h("input", {
        type: "radio",
        name: `lx-${mode}-theme`,
        value: theme.id,
        "aria-label": theme.name,
      });
      // A click on the theme already chosen for the other mode switches to it, and a click
      // fires no change on a checked radio.
      input.addEventListener("click", () => {
        this.pickTheme(theme);
      });
      return { theme, input };
    });
    return buildSwatchGroup(mode, themes);
  }

  /** Sets the theme for its mode; when the other mode is showing, switches to this one. */
  private pickTheme(theme: ThemeDef): void {
    const s = this.host.settings();
    const patch: SettingsPatch = {};
    if ((theme.mode === "light" ? s.lightTheme : s.darkTheme) !== theme.id) {
      patch[theme.mode === "light" ? "lightTheme" : "darkTheme"] = theme.id;
    }
    if (isDark(s, this.host.systemDark()) !== (theme.mode === "dark")) {
      patch.themeMode = theme.mode;
    }
    if (Object.keys(patch).length > 0) {
      this.host.update(patch);
    }
  }

  private fontControl(kind: FontKind, label: string, fonts: string[]): FontControl {
    const select = h("select", { name: `lx-${kind}-font`, "aria-label": label });
    select.append(
      ...fonts.map((font) => h("option", { value: font }, font)),
      h("option", { value: OTHER }, "Other installed font…"),
    );
    const other = h("input", {
      type: "text",
      name: `lx-${kind}-font-other`,
      list: FONT_LIST_ID,
      placeholder: "Installed font name",
      "aria-label": `Installed ${label.toLowerCase()}`,
      spellcheck: "false",
      autocomplete: "off",
    });
    other.hidden = true;

    // Focus comes with the mouse press or the keyboard, before the list opens.
    select.addEventListener("focus", () => {
      void this.loadSystemFonts();
    });
    select.addEventListener("change", () => {
      if (select.value === OTHER) {
        other.hidden = false;
        other.value = "";
        other.focus();
        void this.loadSystemFonts();
      } else {
        other.hidden = true;
        this.setFont(kind, select.value);
      }
    });
    other.addEventListener("change", () => {
      const name = other.value.trim();
      if (name !== "") {
        other.hidden = true;
        this.setFont(kind, name);
      }
    });
    // Left empty, the select goes back to the font in use.
    other.addEventListener("blur", () => {
      if (!other.hidden && other.value.trim() === "") {
        other.hidden = true;
        this.refresh();
      }
    });
    return { kind, select, other };
  }

  private setFont(kind: FontKind, name: string): void {
    this.host.update(kind === "body" ? { bodyFont: name } : { codeFont: name });
  }

  /** Asks for the installed fonts once, for the "Other" suggestions and Cascadia Code. */
  private loadSystemFonts(): Promise<string[]> {
    this.systemFonts ??= this.host.listSystemFonts().then(
      (names) => {
        this.offerFonts(names);
        return names;
      },
      () => [],
    );
    return this.systemFonts;
  }

  private offerFonts(names: string[]): void {
    const ui = this.ui;
    if (!ui) {
      return;
    }
    ui.el
      .querySelector(`#${FONT_LIST_ID}`)
      ?.replaceChildren(...names.map((name) => h("option", { value: name })));
    const code = ui.fonts.find((f) => f.kind === "code")?.select;
    if (code && names.includes(CASCADIA)) {
      const listed = [...code.options].find((o) => o.value === CASCADIA);
      if (listed) {
        // It was shown as the font in use; now it is one of the list.
        listed.classList.remove("custom");
        code.insertBefore(listed, code.options[1] ?? null);
      } else {
        code.insertBefore(h("option", { value: CASCADIA }, CASCADIA), code.options[1] ?? null);
      }
    }
  }
}

/** A labelled row; `kind` is "top" for a tall control (label at the top), "check" for a checkbox. */
function row(label: string, control: HTMLElement, kind?: "top" | "check"): HTMLElement {
  const cls = kind === undefined ? "rp-row" : `rp-row ${kind}`;
  return h("div", { class: cls }, h("span", { class: "rp-label" }, label), control);
}

/** The tiles of a mode's themes, each a page in miniature, and the caption naming the chosen one. */
function buildSwatchGroup(mode: "light" | "dark", themes: SwatchGroup["themes"]): SwatchGroup {
  const label = mode === "light" ? "Light theme" : "Dark theme";
  const tiles = h("div", { class: "swatches", role: "radiogroup", "aria-label": label });
  for (const { theme, input } of themes) {
    const [bg, fg, accent] = theme.swatch;
    const tile = h("label", { class: "swatch", title: theme.name });
    tile.style.setProperty("--sw-bg", bg);
    tile.style.setProperty("--sw-fg", fg);
    tile.style.setProperty("--sw-accent", accent);
    tile.append(input, h("span", { class: "swatch-tile", "aria-hidden": "true" }, "Aa"));
    tiles.append(tile);
  }
  const caption = h("div", { class: "swatch-name", "aria-hidden": "true" });
  return { mode, themes, caption, block: h("div", { class: "swatch-group" }, tiles, caption) };
}

/** Selects `font`, adding it as an option when it isn't one of the listed fonts. */
function syncFont(control: FontControl, font: string): void {
  const { select, other } = control;
  for (const option of select.querySelectorAll<HTMLOptionElement>("option.custom")) {
    if (option.value !== font) {
      option.remove();
    }
  }
  if (![...select.options].some((o) => o.value === font)) {
    const option = h("option", { value: font, class: "custom" }, font);
    select.insertBefore(option, select.options[select.options.length - 1] ?? null);
  }
  // While a name is being typed into "Other", the select stays on it.
  if (other.hidden) {
    select.value = font;
  }
}

/** A slider's value as shown beside it. */
function valueText(input: HTMLInputElement): string {
  const value = Number(input.value);
  switch (input.name) {
    case "lx-font-size":
      return `${String(value)} px`;
    case "lx-line-height":
      return value.toFixed(2);
    default:
      return input.disabled ? "Full" : `${String(value)} ch`;
  }
}
