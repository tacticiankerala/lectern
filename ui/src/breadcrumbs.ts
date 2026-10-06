// The header's breadcrumbs (spec §6): root › folders › file, from the document's `breadcrumbs`.
// Each crumb is a button that opens the chooser (crumb-chooser.ts) on its folder, as the library's
// tree has it; the file's own crumb opens it on the file's folder. A document outside every library
// root has no tree, so its crumbs are plain text.
import { h } from "./dom";
import type { Crumb } from "./generated/Crumb";
import type { RootView } from "./generated/RootView";

/** Paths compare as on Windows: case-insensitively, either separator. */
export function pathKey(path: string): string {
  return path.toLowerCase().replaceAll("/", "\\");
}

/** Whether the path keyed `path` is the folder keyed `dir` or lies below it. */
export function within(path: string, dir: string): boolean {
  return path === dir || path.startsWith(dir.endsWith("\\") ? dir : `${dir}\\`);
}

/** Renders the crumbs; `choose` opens the chooser for a crumb, hanging from its button. */
export function renderBreadcrumbs(
  host: HTMLElement,
  crumbs: Crumb[],
  roots: RootView[],
  choose: (index: number, anchor: HTMLElement) => void,
): void {
  const parts: Node[] = [];
  // All of them or none: the document is in a root's tree or it isn't.
  const doc = pathKey(crumbs[crumbs.length - 1]?.path ?? "");
  const interactive =
    crumbs.length > 1 && roots.some((r) => r.tree !== null && within(doc, pathKey(r.path)));
  crumbs.forEach((crumb, i) => {
    if (i > 0) {
      parts.push(h("span", { class: "crumb-sep", "aria-hidden": "true" }, "›"));
    }
    const attrs: Record<string, string> =
      i === crumbs.length - 1
        ? { class: "crumb current", "aria-current": "page", title: crumb.path }
        : { class: "crumb", title: crumb.path };
    if (!interactive) {
      parts.push(h("span", attrs, crumb.name));
      return;
    }
    const button = h(
      "button",
      {
        ...attrs,
        type: "button",
        "data-index": String(i),
        "aria-haspopup": "dialog",
        "aria-expanded": "false",
      },
      crumb.name,
    );
    button.addEventListener("click", () => {
      choose(i, button);
    });
    parts.push(button);
  });
  host.replaceChildren(...parts);
}
