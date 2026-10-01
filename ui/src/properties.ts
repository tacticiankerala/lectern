// The properties strip above the document: its frontmatter and task count. The status shows as a
// badge, dates as relative text, `prs` numbers as chips, everything else as compact key/value
// pairs. It collapses from its header, and remembers that across documents and launches.
import { h } from "./dom";
import type { DocPayload } from "./generated/DocPayload";
import type { PropValue } from "./generated/PropValue";
import type { Property } from "./generated/Property";

const COLLAPSED_KEY = "lx.properties.collapsed";
const DAY_MS = 86_400_000;
const relative = new Intl.RelativeTimeFormat("en", { numeric: "auto" });

/** Renders `doc`'s properties into `host`, hiding it when there are none. `now` is for tests. */
export function renderProperties(host: HTMLElement, doc: DocPayload, now = Date.now()): void {
  const fm = doc.frontmatter;
  const entries = fm?.kind === "parsed" ? fm.entries : [];
  const hasTasks = doc.tasks.total > 0;
  if (!hasTasks && entries.length === 0 && fm?.kind !== "invalid") {
    host.hidden = true;
    host.replaceChildren();
    return;
  }
  // A text status is the badge; any other status is an ordinary pair.
  const found = entries.find((e) => e.key === "status");
  const status = found?.value.kind === "text" ? found : undefined;
  const head = h("button", { type: "button", class: "props-head" });
  head.append(h("span", { class: "props-chevron", "aria-hidden": "true" }));
  head.append(h("span", { class: "props-label" }, "Properties"));
  if (status?.value.kind === "text") {
    head.append(badge(status.value.value));
  }
  const body = h("div", { class: "props-body" });
  if (fm?.kind === "invalid") {
    head.append(
      h(
        "span",
        {
          class: "props-warning",
          title: `The frontmatter isn't valid YAML: ${fm.error}`,
          role: "img",
        },
        "⚠",
      ),
    );
    body.append(h("pre", { class: "props-raw" }, fm.raw));
  }
  for (const entry of entries) {
    if (entry !== status) {
      body.append(pair(entry, now));
    }
  }
  if (hasTasks) {
    head.append(
      h(
        "span",
        { class: "props-tasks" },
        `${String(doc.tasks.done)} / ${String(doc.tasks.total)} tasks`,
      ),
    );
  }
  const setCollapsed = (collapsed: boolean): void => {
    host.classList.toggle("collapsed", collapsed);
    head.setAttribute("aria-expanded", String(!collapsed));
  };
  setCollapsed(readCollapsed());
  head.addEventListener("click", () => {
    const collapsed = !host.classList.contains("collapsed");
    setCollapsed(collapsed);
    writeCollapsed(collapsed);
  });
  host.replaceChildren(head, body);
  host.hidden = false;
}

function badge(status: string): HTMLElement {
  const slug = status.toLowerCase().replace(/[^a-z0-9]+/g, "-");
  return h("span", { class: `badge status-${slug}` }, status);
}

function pair(entry: Property, now: number): HTMLElement {
  return h(
    "span",
    { class: "prop" },
    h("span", { class: "prop-key" }, entry.key),
    h("span", { class: "prop-value" }, ...value(entry.key, entry.value, now)),
  );
}

function value(key: string, v: PropValue, now: number): (Node | string)[] {
  if (key === "prs") {
    const items = v.kind === "list" ? v.value : [v];
    const numbers = items.filter((item) => item.kind === "number");
    if (numbers.length === items.length) {
      return numbers.map((item) => h("span", { class: "chip" }, `#${item.value}`));
    }
  }
  switch (v.kind) {
    case "date":
    case "dateTime": {
      const text = relativeTime(v.value, v.kind, now);
      return [text === null ? v.value : h("time", { datetime: v.value, title: v.value }, text)];
    }
    case "bool":
      return [v.value ? "yes" : "no"];
    case "list":
      return v.value.flatMap((item, i) => [...(i > 0 ? [", "] : []), ...value("", item, now)]);
    default:
      return [v.value];
  }
}

/** "2 days ago", "in 3 weeks", "3 hours ago": calendar days for dates, hours for recent times. */
export function relativeTime(text: string, kind: "date" | "dateTime", now: number): string | null {
  const then = parse(text, kind);
  if (then === null) {
    return null;
  }
  if (kind === "dateTime") {
    const minutes = Math.round((then - now) / 60_000);
    if (Math.abs(minutes) < 60) {
      return relative.format(minutes, "minute");
    }
    const hours = Math.round(minutes / 60);
    if (Math.abs(hours) < 24) {
      return relative.format(hours, "hour");
    }
  }
  const days = Math.round((startOfDay(then) - startOfDay(now)) / DAY_MS);
  const size = Math.abs(days);
  if (size < 7) {
    return relative.format(days, "day");
  }
  if (size < 30) {
    return relative.format(Math.round(days / 7), "week");
  }
  if (size < 365) {
    return relative.format(Math.round(days / 30), "month");
  }
  return relative.format(Math.round(days / 365), "year");
}

/** A date is a local calendar day; a time without a zone is local too. */
function parse(text: string, kind: "date" | "dateTime"): number | null {
  const ms =
    kind === "date"
      ? new Date(
          Number(text.slice(0, 4)),
          Number(text.slice(5, 7)) - 1,
          Number(text.slice(8, 10)),
        ).getTime()
      : new Date(`${text.slice(0, 10)}T${text.slice(11)}`).getTime();
  return Number.isNaN(ms) ? null : ms;
}

function startOfDay(ms: number): number {
  const d = new Date(ms);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

function readCollapsed(): boolean {
  try {
    return localStorage.getItem(COLLAPSED_KEY) === "1";
  } catch {
    return false;
  }
}

function writeCollapsed(collapsed: boolean): void {
  try {
    localStorage.setItem(COLLAPSED_KEY, collapsed ? "1" : "0");
  } catch {
    // Storage may be unavailable; the strip then simply starts expanded.
  }
}
