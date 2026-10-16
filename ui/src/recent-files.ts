// The recent files menu (the header's clock, or Ctrl+H): which of the workspace's recent files it
// lists and how it names each. Every recent file but the one on screen, newest first, up to the
// `recentInMenu` setting; the first ten have the number keys 1 to 9, then 0. Each is named by its
// title and its folder: below its library root, `/`-separated as quick open has it (the root's own
// name at its top), else the whole folder, its middle cut when long. actions.ts builds the menu.
import { pathKey, within } from "./breadcrumbs";
import { samePath } from "./dom";
import type { RecentEntry } from "./generated/RecentEntry";
import type { RootView } from "./generated/RootView";

/** How many the menu lists, as Rust keeps the setting. */
export const RECENT_IN_MENU = { min: 5, max: 20 } as const;
/** The number keys, in order. */
const DIGITS = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
/** The longest folder a row shows, in characters. */
const FOLDER_CHARS = 48;

/** A recent file, as the menu lists it. */
export interface RecentChoice {
  path: string;
  title: string;
  folder: string;
  /** The number key that opens it: "1" to "9", then "0"; none past the tenth. */
  key: string | null;
}

/** The recent files the menu lists: every one but `current`, up to `max`. */
export function recentChoices(
  recent: RecentEntry[],
  current: string | null,
  roots: Pick<RootView, "path" | "name">[],
  max: number,
): RecentChoice[] {
  return recent
    .filter((entry) => current === null || !samePath(entry.path, current))
    .slice(0, max)
    .map((entry, i) => ({
      path: entry.path,
      title: entry.title,
      folder: middleTruncate(folderOf(entry.path, roots), FOLDER_CHARS),
      key: DIGITS[i] ?? null,
    }));
}

/**
 * The folder `path` is in: below its root, `/`-separated (the root's name at its top); outside
 * every root, the whole folder.
 */
function folderOf(path: string, roots: Pick<RootView, "path" | "name">[]): string {
  const key = pathKey(path);
  const root = roots.find((r) => within(key, pathKey(r.path)));
  if (root) {
    const below = path
      .slice(root.path.length)
      .replace(/^[\\/]+/, "")
      .split(/[\\/]/);
    return below.length > 1 ? below.slice(0, -1).join("/") : root.name;
  }
  const end = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  const folder = path.slice(0, end);
  // A drive's root keeps its separator: `D:\`, not `D:`.
  return folder.endsWith(":") ? path.slice(0, end + 1) : folder;
}

/** `text` cut to `max` characters by dropping its middle, which an ellipsis stands for. */
function middleTruncate(text: string, max: number): string {
  const chars = Array.from(new Intl.Segmenter().segment(text), (s) => s.segment);
  if (chars.length <= max) {
    return text;
  }
  const head = Math.floor((max - 1) / 2);
  const tail = max - 1 - head;
  return `${chars.slice(0, head).join("")}…${chars.slice(chars.length - tail).join("")}`;
}
