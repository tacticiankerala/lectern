import { beforeEach, describe, expect, it, vi } from "vitest";
import appRs from "../../src-tauri/src/app.rs?raw";
import fontsCss from "../styles/fonts.css?raw";
import themesCss from "../styles/themes.css?raw";
import { DEFAULT_SETTINGS } from "../src/app";
import type { Backend } from "../src/backend";
import type { Settings } from "../src/generated/Settings";
import {
  THEMES,
  applySettings,
  bumpFontSize,
  effectiveTheme,
  fontStack,
  loadFonts,
  loadRememberedFonts,
  toggleThemeMode,
} from "../src/themes";

const defaults: Settings = DEFAULT_SETTINGS;

const IDS = ["paper", "daylight", "sepia", "latte", "graphite", "midnight", "nord", "mocha"];

const VARIABLES = [
  "bg",
  "bg-elev",
  "fg",
  "fg-muted",
  "accent",
  "border",
  "code-bg",
  "code-fg",
  "link",
  "selection",
  "mark",
  "badge-active",
  "badge-blocked",
  "badge-parked",
  "badge-done",
  "comment",
  "comment-focus",
  "comment-dot",
];
const SYNTAX = [
  "keyword",
  "string",
  "number",
  "comment",
  "function",
  "type",
  "variable",
  "constant",
  "operator",
  "punctuation",
  "tag",
  "attribute",
  "inserted",
  "deleted",
].map((token) => `syn-${token}`);

/** Each `html[data-theme="…"]` block of themes.css: its declarations, by property name. */
function parseThemes(css: string): Map<string, Map<string, string>> {
  const themes = new Map<string, Map<string, string>>();
  for (const block of css.matchAll(/html\[data-theme="([a-z]+)"\]\s*\{([^}]*)\}/g)) {
    const decls = new Map<string, string>();
    for (const decl of (block[2] ?? "").matchAll(/(--[\w-]+|color-scheme)\s*:\s*([^;]+);/g)) {
      decls.set(decl[1] ?? "", (decl[2] ?? "").trim());
    }
    themes.set(block[1] ?? "", decls);
  }
  return themes;
}

/** WCAG 2 relative luminance of `#rrggbb`. */
function luminance(hex: string): number {
  const channel = (i: number): number => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return ((hi ?? 0) + 0.05) / ((lo ?? 0) + 0.05);
}

const HEX = /^#[0-9a-f]{6}$/;

/** A theme's hex value for `name`, failing the test when it is missing or not `#rrggbb`. */
function hexOf(theme: Map<string, string>, name: string): string {
  const value = theme.get(`--${name}`) ?? "";
  expect(value, `--${name}`).toMatch(HEX);
  return value;
}

/** An RGB colour and its opacity, 0 to 1. */
type Rgba = [number, number, number, number];

const RGB = /^rgba?\(\s*(\d+)[\s,]+(\d+)[\s,]+(\d+)\s*(?:[/,]\s*([\d.]+)(%?))?\s*\)$/;

/**
 * A theme's colour for `name`, from `#rrggbb` or `rgb(r g b / a)` (or the comma form), failing
 * the test when it is missing or neither.
 */
function rgbaOf(theme: Map<string, string>, name: string): Rgba {
  const value = theme.get(`--${name}`) ?? "";
  if (HEX.test(value)) {
    const hex = hexOf(theme, name);
    return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)).concat(1) as Rgba;
  }
  const match = RGB.exec(value);
  expect(match, `--${name}: ${value}`).not.toBeNull();
  const [, r, g, b, alpha, percent] = match ?? [];
  const a = alpha === undefined ? 1 : Number(alpha) / (percent === "%" ? 100 : 1);
  return [Number(r), Number(g), Number(b), a];
}

/** `color` painted over the opaque `bg`, as the browser blends it, as `#rrggbb`. */
function over([r, g, b, a]: Rgba, bg: string): string {
  return `#${[r, g, b]
    .map((c, i) => {
      const under = parseInt(bg.slice(1 + 2 * i, 3 + 2 * i), 16);
      return Math.round(a * c + (1 - a) * under)
        .toString(16)
        .padStart(2, "0");
    })
    .join("")}`;
}

const themes = parseThemes(themesCss);

function chromeBackend() {
  const setChromeColors = vi.fn(() => Promise.resolve());
  return { backend: { setChromeColors } as unknown as Backend, setChromeColors };
}

describe("themes", () => {
  it("system mode picks dark theme when OS dark", () => {
    expect(effectiveTheme({ ...defaults, themeMode: "system" }, true).id).toBe("graphite");
    expect(effectiveTheme({ ...defaults, themeMode: "system" }, false).id).toBe("paper");
  });

  it("an explicit mode ignores the OS and uses that theme of the pair", () => {
    const s: Settings = { ...defaults, lightTheme: "sepia", darkTheme: "nord" };
    expect(effectiveTheme({ ...s, themeMode: "light" }, true).id).toBe("sepia");
    expect(effectiveTheme({ ...s, themeMode: "dark" }, false).id).toBe("nord");
  });

  it("toggle flips to the pair", () => {
    expect(toggleThemeMode({ ...defaults, themeMode: "system" }, false)).toEqual({
      themeMode: "dark",
    });
    expect(toggleThemeMode({ ...defaults, themeMode: "system" }, true)).toEqual({
      themeMode: "light",
    });
    expect(toggleThemeMode({ ...defaults, themeMode: "dark" }, true)).toEqual({
      themeMode: "light",
    });
    expect(toggleThemeMode({ ...defaults, themeMode: "light" }, true)).toEqual({
      themeMode: "dark",
    });
  });

  it("font size clamps", () => {
    expect(bumpFontSize({ ...defaults, fontSize: 32 }, 1)).toEqual({ fontSize: 32 });
    expect(bumpFontSize({ ...defaults, fontSize: 12 }, -1)).toEqual({ fontSize: 12 });
    expect(bumpFontSize({ ...defaults, fontSize: 20 }, 1)).toEqual({ fontSize: 21 });
    expect(bumpFontSize(defaults, 0)).toEqual({ fontSize: 18 });
    expect(bumpFontSize({ ...defaults, fontSize: 25 }, 0)).toEqual({ fontSize: 18 });
  });

  it("lists the eight themes, light then dark", () => {
    expect(THEMES.map((t) => t.id)).toEqual(IDS);
    const modes = THEMES.map((t) => t.mode);
    expect(modes).toEqual(["light", "light", "light", "light", "dark", "dark", "dark", "dark"]);
  });

  it("every theme defines every variable", () => {
    expect([...themes.keys()].sort()).toEqual([...IDS].sort());
    for (const id of IDS) {
      const theme = themes.get(id);
      for (const name of [...VARIABLES, ...SYNTAX]) {
        expect(theme?.get(`--${name}`), `${id} --${name}`).toBeTruthy();
      }
      expect(theme?.get("color-scheme"), `${id} color-scheme`).toMatch(/^(light|dark)$/);
    }
  });

  it("contrast meets thresholds", () => {
    for (const id of IDS) {
      const theme = themes.get(id) ?? new Map<string, string>();
      const bg = hexOf(theme, "bg");
      const codeBg = hexOf(theme, "code-bg");
      expect(contrast(hexOf(theme, "fg"), bg), `${id} body`).toBeGreaterThanOrEqual(7);
      expect(contrast(hexOf(theme, "fg-muted"), bg), `${id} muted`).toBeGreaterThanOrEqual(4.5);
      for (const name of SYNTAX) {
        expect(contrast(hexOf(theme, name), codeBg), `${id} --${name}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("comment highlights keep text readable", () => {
    for (const id of IDS) {
      const theme = themes.get(id) ?? new Map<string, string>();
      const bg = hexOf(theme, "bg");
      const fg = hexOf(theme, "fg");
      for (const name of ["comment", "comment-focus"]) {
        const under = over(rgbaOf(theme, name), bg);
        expect(contrast(fg, under), `${id} text on --${name}`).toBeGreaterThanOrEqual(4.5);
      }
      const dot = over(rgbaOf(theme, "comment-dot"), bg);
      expect(contrast(dot, bg), `${id} --comment-dot`).toBeGreaterThanOrEqual(3);
      // Comments and find in page never share a colour.
      expect(theme.get("--comment"), `${id} --comment`).not.toBe(theme.get("--mark"));
    }
  });

  it("THEMES matches themes.css: background, text, mode and swatch", () => {
    for (const def of THEMES) {
      const theme = themes.get(def.id) ?? new Map<string, string>();
      expect(def.bg, def.id).toBe(theme.get("--bg"));
      expect(def.fg, def.id).toBe(theme.get("--fg"));
      expect(def.mode, def.id).toBe(theme.get("color-scheme"));
      expect(def.swatch, def.id).toEqual([def.bg, def.fg, theme.get("--accent")]);
    }
  });

  it("the Rust window table matches themes.css", () => {
    const rows = [
      ...appRs.matchAll(/ThemeId::(\w+) => \("(#[0-9a-f]{6})", "(#[0-9a-f]{6})", (true|false)\)/g),
    ];
    expect(rows.map((row) => row[1]?.toLowerCase()).sort()).toEqual([...IDS].sort());
    for (const [, name, bg, fg, dark] of rows) {
      const id = name?.toLowerCase() ?? "";
      const theme = themes.get(id);
      expect(bg, id).toBe(theme?.get("--bg"));
      expect(fg, id).toBe(theme?.get("--fg"));
      expect(dark === "true" ? "dark" : "light", id).toBe(theme?.get("color-scheme"));
    }
  });
});

describe("applySettings", () => {
  beforeEach(() => {
    document.documentElement.removeAttribute("data-theme");
    document.documentElement.removeAttribute("style");
    document.documentElement.className = "";
  });

  it("sets the theme, the reading variables and code wrap, and colours the title bar", () => {
    const { backend, setChromeColors } = chromeBackend();
    applySettings(
      {
        ...defaults,
        fontSize: 20,
        lineHeight: 1.8,
        measure: 90,
        bodyFont: "Literata",
        codeWrap: true,
      },
      false,
      backend,
    );
    const root = document.documentElement;
    expect(root.dataset.theme).toBe("paper");
    expect(root.style.getPropertyValue("--font-size")).toBe("20px");
    expect(root.style.getPropertyValue("--line-height")).toBe("1.8");
    expect(root.style.getPropertyValue("--measure")).toBe("90ch");
    expect(root.style.getPropertyValue("--body-font")).toMatch(/^"Literata", /);
    expect(root.style.getPropertyValue("--code-font")).toMatch(/^"JetBrains Mono", /);
    expect(root.classList.contains("code-wrap")).toBe(true);
    expect(setChromeColors).toHaveBeenCalledWith("#f8f5ee", "#2b2a27", false);
  });

  it("full width lifts the measure", () => {
    const { backend } = chromeBackend();
    applySettings({ ...defaults, measure: "full" }, false, backend);
    expect(document.documentElement.style.getPropertyValue("--measure")).toBe("none");
  });

  it("colours the title bar again only when the theme changes", () => {
    const { backend, setChromeColors } = chromeBackend();
    applySettings(defaults, false, backend);
    applySettings({ ...defaults, fontSize: 24 }, false, backend);
    expect(setChromeColors).toHaveBeenCalledTimes(1);
    applySettings(defaults, true, backend);
    expect(setChromeColors).toHaveBeenCalledTimes(2);
    expect(setChromeColors).toHaveBeenLastCalledWith("#1e1f22", "#d7d8db", true);
    expect(document.documentElement.dataset.theme).toBe("graphite");
  });
});

describe("fontStack", () => {
  it("quotes the family and falls back to the system fonts", () => {
    expect(fontStack("Inter", "body")).toMatch(/^"Inter", "Segoe UI", .*"Segoe UI Emoji"$/);
    expect(fontStack("JetBrains Mono", "code")).toMatch(/^"JetBrains Mono", .*monospace$/);
  });

  it("falls back to a serif for the serif faces", () => {
    expect(fontStack("Source Serif 4", "body")).toMatch(/^"Source Serif 4", Georgia, .*serif/);
  });

  it("strips quotes and backslashes from a font name", () => {
    expect(fontStack('Evil", red', "body")).toMatch(/^"Evil, red", /);
  });
});

describe("font loading", () => {
  const load = vi.fn(() => Promise.resolve([]));
  beforeEach(() => {
    load.mockClear();
    localStorage.clear();
    Object.defineProperty(document, "fonts", { value: { load }, configurable: true });
  });

  it("loads only the selected bundled faces", async () => {
    await loadFonts({ ...defaults, bodyFont: "Literata", codeFont: "Consolas" });
    expect(load.mock.calls).toEqual([['1em "Literata"']]);
    load.mockClear();
    await loadFonts(defaults);
    expect(load.mock.calls).toEqual([['1em "JetBrains Mono"']]);
  });

  it("remembers the selected bundled faces for the next launch to start on", async () => {
    applySettings({ ...defaults, bodyFont: "Inter" }, false, chromeBackend().backend);
    await loadRememberedFonts();
    expect(load.mock.calls).toEqual([['1em "Inter"'], ['1em "JetBrains Mono"']]);
  });

  it("starts with nothing remembered, and ignores anything but bundled faces", async () => {
    await loadRememberedFonts();
    localStorage.setItem("lx-fonts", '["Comic Sans MS", "Literata", 42]');
    await loadRememberedFonts();
    localStorage.setItem("lx-fonts", "not json");
    await loadRememberedFonts();
    expect(load.mock.calls).toEqual([['1em "Literata"']]);
  });
});

describe("fonts.css", () => {
  const faces = [...fontsCss.matchAll(/@font-face\s*\{([^}]*)\}/g)].map((m) => {
    const body = m[1] ?? "";
    return {
      family: /font-family:\s*"([^"]+)"/.exec(body)?.[1] ?? "",
      ranged: /font-weight:\s*\d+\s+\d+\s*;/.test(body),
      locals: [...body.matchAll(/local\("([^"]+)"\)/g)].map((l) => l[1] ?? ""),
    };
  });

  it("declares an upright and an italic variable face for each bundled family", () => {
    expect(faces).toHaveLength(10);
    expect(faces.every((face) => face.ranged)).toBe(true);
  });

  // A static install answering a weight range would render every weight from its one face, so
  // bold would be lost: only a name that only the variable build has may stand in.
  it("takes an installed copy of a variable face only by a variable-only name", () => {
    for (const face of faces.filter((f) => f.ranged)) {
      for (const name of face.locals) {
        expect(name, `${face.family}: local("${name}")`).toMatch(/Variable/);
      }
    }
  });

  it("uses only the bundled files for families whose names a static build shares", () => {
    for (const family of ["Atkinson Hyperlegible Next", "Literata", "JetBrains Mono"]) {
      const locals = faces.filter((f) => f.family === family).flatMap((f) => f.locals);
      expect(locals, family).toEqual([]);
    }
  });
});
