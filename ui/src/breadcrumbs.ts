// The header's breadcrumbs (spec §6): root › folders › file, from the document's `breadcrumbs`.
// A folder with a README opens it; one without is revealed in the library sidebar.
import { h } from "./dom";
import type { Crumb } from "./generated/Crumb";

export interface CrumbActions {
  open(readme: string): void;
  reveal(folder: string): void;
}

export function renderBreadcrumbs(host: HTMLElement, crumbs: Crumb[], actions: CrumbActions): void {
  const parts: Node[] = [];
  crumbs.forEach((crumb, i) => {
    if (i > 0) {
      parts.push(h("span", { class: "crumb-sep", "aria-hidden": "true" }, "›"));
    }
    if (i === crumbs.length - 1) {
      parts.push(
        h(
          "span",
          { class: "crumb current", "aria-current": "page", title: crumb.path },
          crumb.name,
        ),
      );
      return;
    }
    const button = h(
      "button",
      {
        type: "button",
        class: crumb.readme === null ? "crumb" : "crumb has-readme",
        title: crumb.readme ?? crumb.path,
      },
      crumb.name,
    );
    const readme = crumb.readme;
    button.addEventListener("click", () => {
      if (readme === null) {
        actions.reveal(crumb.path);
      } else {
        actions.open(readme);
      }
    });
    parts.push(button);
  });
  host.replaceChildren(...parts);
}
