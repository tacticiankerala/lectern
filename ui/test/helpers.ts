// Small fixtures and fakes shared by the Vitest suites.
import { FakeBackend, type Fixtures } from "../dev/backend-fake";
import type { DocPayload } from "../src/generated/DocPayload";
import type { FollowResult } from "../src/generated/FollowResult";
import type { FollowTarget } from "../src/generated/FollowTarget";
import type { Frontmatter } from "../src/generated/Frontmatter";
import type { OpenResult } from "../src/generated/OpenResult";
import type { RenderedDoc } from "../src/generated/RenderedDoc";
import type { StartupPayload } from "../src/generated/StartupPayload";
import type { TaskStats } from "../src/generated/TaskStats";

export const ROOT = "C:\\V";
export const A = "C:\\V\\a.md";
export const B = "C:\\V\\notes\\b.md";
export const C = "C:\\V\\notes\\c.md";

export function rendered(title: string, html: string): RenderedDoc {
  return {
    html,
    outline: [],
    frontmatter: null,
    tasks: { done: 0, total: 0 },
    title,
    wordCount: 0,
    hasUnresolvedWikilinks: false,
  };
}

export function fixtures(docs: Record<string, RenderedDoc> = {}): Fixtures {
  return {
    root: ROOT,
    tree: { name: "V", path: ROOT, isDir: true, children: [], readme: null, status: null },
    candidates: [],
    docs: {
      [A]: rendered("Aye", "<h1 id='aye'>Aye</h1>"),
      [B]: rendered("Bee", "<h1 id='bee'>Bee</h1>"),
      [C]: rendered("Sea", "<h1 id='sea'>Sea</h1>"),
      ...docs,
    },
  };
}

export function docPayload(
  frontmatter: Frontmatter | null,
  tasks: TaskStats = { done: 0, total: 0 },
): DocPayload {
  return {
    path: A,
    title: "Aye",
    html: "",
    outline: [],
    frontmatter,
    tasks,
    wordCount: 0,
    mtimeMs: 0,
    lossy: false,
    position: null,
    breadcrumbs: [],
    rootPath: ROOT,
  };
}

export interface Deferred {
  promise: Promise<void>;
  resolve(): void;
}

export function deferred(): Deferred {
  let resolve = (): void => undefined;
  const promise = new Promise<void>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/**
 * A fake whose startup waits for `startupGate`; whose opens of `slowPath` are answered, as they
 * were when asked, only once `openGate` opens; and whose `follow` waits for `followGate` while
 * `slowFollow` is set. `overrides` answers opens of a path with a given result.
 */
export class GatedBackend extends FakeBackend {
  readonly startupGate = deferred();
  readonly openGate = deferred();
  readonly followGate = deferred();
  slowPath: string | null = null;
  slowFollow = false;
  notice: string | null = null;
  overrides: Record<string, OpenResult> = {};

  override async startup(): Promise<StartupPayload> {
    await this.startupGate.promise;
    return { ...(await super.startup()), startupNotice: this.notice };
  }

  override async openDocument(path: string): Promise<OpenResult> {
    const result = this.overrides[path] ?? (await super.openDocument(path));
    if (path === this.slowPath) {
      await this.openGate.promise;
    }
    return result;
  }

  override async follow(target: FollowTarget): Promise<FollowResult> {
    if (this.slowFollow) {
      await this.followGate.promise;
    }
    return super.follow(target);
  }
}

/** Lets pending promises and a few frames run. */
export function settle(ms = 50): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** A fresh `#lx-app` root in the document. */
export function appRoot(): HTMLElement {
  document.body.innerHTML = "<div id='lx-app' class='layout'></div>";
  const root = document.getElementById("lx-app");
  if (!root) {
    throw new Error("no root");
  }
  return root;
}
