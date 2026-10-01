// Themes and reading settings: which theme is in effect, and applying the settings to the page as
// html[data-theme], CSS variables and the title-bar colours. The colours themselves live in
// styles/themes.css; `bg` and `fg` here mirror it for the swatches and the title bar (tests pin
// them to the stylesheet).
import type { Backend } from "./backend";
import type { Settings } from "./generated/Settings";
import type { SettingsPatch } from "./generated/SettingsPatch";
import type { ThemeId } from "./generated/ThemeId";

export interface ThemeDef {
  id: ThemeId;
  name: string;
  mode: "light" | "dark";
  bg: string;
  fg: string;
  /** Background, text and accent, for the reading panel's swatch. */
  swatch: [string, string, string];
}

function theme(
  id: ThemeId,
  name: string,
  mode: "light" | "dark",
  [bg, fg, accent]: [string, string, string],
): ThemeDef {
  return { id, name, mode, bg, fg, swatch: [bg, fg, accent] };
}

/** Light themes, then dark, in the reading panel's order. */
export const THEMES: ThemeDef[] = [
  theme("paper", "Paper", "light", ["#f8f5ee", "#2b2a27", "#9a5b2e"]),
  theme("daylight", "Daylight", "light", ["#ffffff", "#1f2328", "#0969da"]),
  theme("sepia", "Sepia", "light", ["#f4ecd8", "#433422", "#8b4513"]),
  theme("latte", "Catppuccin Latte", "light", ["#eff1f5", "#4c4f69", "#8839ef"]),
  theme("graphite", "Graphite", "dark", ["#1e1f22", "#d7d8db", "#7aa2f7"]),
  theme("midnight", "Midnight", "dark", ["#000000", "#cfcfd4", "#8ab4f8"]),
  theme("nord", "Nord", "dark", ["#2e3440", "#e5e9f0", "#88c0d0"]),
  theme("mocha", "Catppuccin Mocha", "dark", ["#1e1e2e", "#cdd6f4", "#cba6f7"]),
];

export const FONT_SIZE = { min: 12, max: 32, default: 18 } as const;
export const LINE_HEIGHT = { min: 1.3, max: 2, step: 0.05 } as const;
export const MEASURE = { min: 50, max: 120 } as const;

/** Fonts that ship with Lectern (styles/fonts.css). */
const BUNDLED_FONTS = new Set([
  "Inter",
  "Atkinson Hyperlegible Next",
  "Literata",
  "Source Serif 4",
  "JetBrains Mono",
]);

const SERIF_FONTS = new Set(["Literata", "Source Serif 4", "Georgia", "Sitka Text", "Cambria"]);
const SANS_FALLBACK = '"Segoe UI", system-ui, sans-serif, "Segoe UI Emoji"';
const SERIF_FALLBACK = 'Georgia, "Times New Roman", serif, "Segoe UI Emoji"';
const CODE_FALLBACK = '"Cascadia Code", Consolas, ui-monospace, monospace';

function themeById(id: ThemeId): ThemeDef {
  return THEMES.find((t) => t.id === id) ?? (THEMES[0] as ThemeDef);
}

/** Whether the dark theme of the pair is in effect. */
export function isDark(s: Settings, systemDark: boolean): boolean {
  return s.themeMode === "dark" || (s.themeMode === "system" && systemDark);
}

export function effectiveTheme(s: Settings, systemDark: boolean): ThemeDef {
  return themeById(isDark(s, systemDark) ? s.darkTheme : s.lightTheme);
}

/** A font family as a CSS stack: the family quoted, then fallbacks of its kind. */
export function fontStack(name: string, kind: "body" | "code"): string {
  const family = `"${name.replace(/["\\]/g, "")}"`;
  if (kind === "code") {
    return `${family}, ${CODE_FALLBACK}`;
  }
  return `${family}, ${SERIF_FONTS.has(name) ? SERIF_FALLBACK : SANS_FALLBACK}`;
}

/** The title-bar colours last sent, per backend, so unchanged themes don't repeat the call. */
const chromeSent = new WeakMap<Backend, string>();

/**
 * Applies the reading settings to the page: the theme, the reading variables and code wrap. The
 * title bar follows the theme; it is only told when the theme changes.
 */
export function applySettings(s: Settings, systemDark: boolean, backend: Backend): void {
  const root = document.documentElement;
  const theme = effectiveTheme(s, systemDark);
  root.dataset.theme = theme.id;
  root.style.setProperty("--font-size", `${String(s.fontSize)}px`);
  root.style.setProperty("--line-height", String(s.lineHeight));
  root.style.setProperty("--measure", s.measure === "full" ? "none" : `${String(s.measure)}ch`);
  root.style.setProperty("--body-font", fontStack(s.bodyFont, "body"));
  root.style.setProperty("--code-font", fontStack(s.codeFont, "code"));
  root.classList.toggle("code-wrap", s.codeWrap);
  rememberFonts(s);

  const dark = theme.mode === "dark";
  const key = `${theme.bg} ${theme.fg} ${String(dark)}`;
  if (chromeSent.get(backend) !== key) {
    chromeSent.set(backend, key);
    backend.setChromeColors(theme.bg, theme.fg, dark).catch((e: unknown) => {
      console.warn(e);
    });
  }
}

/** Where the bundled fonts last selected are remembered, for the next launch to start on. */
const FONTS_KEY = "lx-fonts";

/** The bundled fonts among the selected ones. */
function bundledFonts(s: Settings): string[] {
  return [s.bodyFont, s.codeFont].filter((name) => BUNDLED_FONTS.has(name));
}

/**
 * Loads bundled faces (upright; italics load when used), so the first paint already has them.
 * System fonts need nothing.
 */
function load(families: string[]): Promise<void> {
  if (typeof document === "undefined" || !("fonts" in document)) {
    return Promise.resolve();
  }
  return Promise.all(families.map((name) => document.fonts.load(`1em "${name}"`))).then(
    () => undefined,
    () => undefined,
  );
}

/** Loads the selected bundled faces. */
export function loadFonts(s: Settings): Promise<void> {
  return load(bundledFonts(s));
}

/**
 * Loads the bundled faces selected when Lectern last ran, before the settings arrive: decoding a
 * face takes long enough on Windows that it is best done while the startup payload is on its way.
 */
export function loadRememberedFonts(): Promise<void> {
  let families: unknown = [];
  try {
    families = JSON.parse(localStorage.getItem(FONTS_KEY) ?? "[]");
  } catch {
    // No storage, or something else in it: the fonts load once the settings arrive.
  }
  return Array.isArray(families)
    ? load(families.filter((name): name is string => BUNDLED_FONTS.has(String(name))))
    : Promise.resolve();
}

function rememberFonts(s: Settings): void {
  const value = JSON.stringify(bundledFonts(s));
  try {
    if (localStorage.getItem(FONTS_KEY) !== value) {
      localStorage.setItem(FONTS_KEY, value);
    }
  } catch {
    // Without storage the next launch loads its fonts a little later; nothing else changes.
  }
}

/** One size step up or down, within 12–32; a delta of 0 resets to 18. */
export function bumpFontSize(s: Settings, delta: number): SettingsPatch {
  if (delta === 0) {
    return { fontSize: FONT_SIZE.default };
  }
  return { fontSize: Math.min(FONT_SIZE.max, Math.max(FONT_SIZE.min, s.fontSize + delta)) };
}

/** Switches to the other theme of the pair, setting the mode explicitly. */
export function toggleThemeMode(s: Settings, systemDark: boolean): SettingsPatch {
  return { themeMode: isDark(s, systemDark) ? "light" : "dark" };
}
