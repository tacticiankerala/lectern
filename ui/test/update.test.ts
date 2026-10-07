import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeBackend } from "../dev/backend-fake";
import { App } from "../src/app";
import type { UpdateInfo } from "../src/generated/UpdateInfo";
import { appRoot, deferred, fixtures, settle } from "./helpers";

/** As app.ts and update.ts have them. */
const CHECK_DELAY_MS = 5000;
const DAY_MS = 24 * 60 * 60 * 1000;
const LAST_CHECK_KEY = "lx-update-checked";

const INSTALLED: UpdateInfo = { version: "0.2.0", notes: null, portable: false };
const PORTABLE: UpdateInfo = { version: "0.2.0", notes: null, portable: true };

async function launch(setup: (fake: FakeBackend) => void = () => undefined) {
  const fake = new FakeBackend(fixtures());
  setup(fake);
  const app = new App(fake, appRoot());
  await app.start();
  return { fake, app };
}

/** Runs the clock past the automatic check's delay and lets a check that started finish. */
async function pastCheckDelay(): Promise<void> {
  await vi.advanceTimersByTimeAsync(CHECK_DELAY_MS);
  // The update module loads on first use.
  await vi.dynamicImportSettled();
  await settle(20);
}

function pill(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>("#lx-header-actions .update-pill");
}

function toasts(): string[] {
  return [...document.querySelectorAll("#lx-toasts .toast")].map((t) => t.textContent);
}

async function menuItem(label: string): Promise<HTMLButtonElement> {
  document.querySelector<HTMLButtonElement>("#lx-more-btn")?.click();
  return vi.waitFor(() => {
    const item = [...document.querySelectorAll<HTMLButtonElement>(".menu-item")].find(
      (b) => b.querySelector(".menu-label")?.textContent === label,
    );
    if (!item) throw new Error(`no ${label} item`);
    return item;
  });
}

describe("update checks", () => {
  beforeEach(() => {
    localStorage.clear();
    // Real time keeps flowing for startup's frames and font wait; tests jump past the delay.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"], shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-10-02T09:00:00Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("checks 5 s after startup", async () => {
    const { fake } = await launch();
    await vi.advanceTimersByTimeAsync(CHECK_DELAY_MS - 500);
    await settle(20);
    expect(fake.updateCalls).toEqual([]);
    await pastCheckDelay();
    expect(fake.updateCalls).toEqual(["check"]);
  });

  it("doesn't check when automatic checks are off", async () => {
    const { fake } = await launch((f) => void f.setSettings({ autoUpdate: false }));
    await pastCheckDelay();
    await vi.advanceTimersByTimeAsync(DAY_MS);
    await settle(20);
    expect(fake.updateCalls).toEqual([]);
  });

  it("checks automatically at most once a day", async () => {
    const first = await launch();
    await pastCheckDelay();
    expect(first.fake.updateCalls).toEqual(["check"]);

    vi.setSystemTime(Date.now() + DAY_MS - 60_000);
    const sameDay = await launch();
    await pastCheckDelay();
    expect(sameDay.fake.updateCalls).toEqual([]);

    vi.setSystemTime(Date.now() + 2 * 60_000);
    const nextDay = await launch();
    await pastCheckDelay();
    expect(nextDay.fake.updateCalls).toEqual(["check"]);
  });

  it("keeps a failed automatic check quiet and tries again at the next launch", async () => {
    const first = await launch((f) => {
      f.updateError = "couldn't reach GitHub";
    });
    await pastCheckDelay();
    expect(first.fake.updateCalls).toEqual(["check"]);
    expect(toasts()).toEqual([]);
    expect(pill()).toBeNull();
    expect(localStorage.getItem(LAST_CHECK_KEY)).toBeNull();

    const next = await launch();
    await pastCheckDelay();
    expect(next.fake.updateCalls).toEqual(["check"]);
  });

  it("shows an update as a pill that installs it", async () => {
    const { fake } = await launch((f) => {
      f.update = INSTALLED;
    });
    await pastCheckDelay();
    const button = pill();
    expect(button?.textContent).toBe("Update to v0.2.0");
    expect(toasts()).toEqual([]);
    button?.click();
    expect(button?.textContent).toBe("Installing…");
    await settle(20);
    expect(fake.updateCalls).toEqual(["check", "install"]);
  });

  it("keeps the pill disabled while installing, even through another check", async () => {
    const { fake } = await launch((f) => {
      f.update = INSTALLED;
    });
    await pastCheckDelay();
    const download = deferred();
    const install = vi
      .spyOn(fake, "installUpdate")
      .mockReturnValue(download.promise.then((): string[] => []));
    pill()?.click();
    expect(pill()?.textContent).toBe("Installing…");
    expect(pill()?.disabled).toBe(true);

    (await menuItem("Check for updates")).click();
    await vi.waitFor(() => {
      expect(toasts()).toContain("Lectern 0.2.0 is available.");
    });
    expect(pill()?.textContent).toBe("Installing…");
    expect(pill()?.disabled).toBe(true);
    pill()?.click();
    expect(install).toHaveBeenCalledTimes(1);

    // An install that comes back (it failed, or Lectern didn't exit) offers the update again.
    download.resolve();
    await settle(20);
    expect(pill()?.textContent).toBe("Update to v0.2.0");
    expect(pill()?.disabled).toBe(false);
  });

  it("offers a portable copy the download instead", async () => {
    const { fake } = await launch((f) => {
      f.update = PORTABLE;
    });
    await pastCheckDelay();
    const button = pill();
    expect(button?.textContent).toBe("Download v0.2.0");
    button?.click();
    await settle(20);
    expect(fake.updateCalls).toEqual(["check", "install"]);
    expect(button?.textContent).toBe("Download v0.2.0");
  });

  it("Check for updates always checks and says when Lectern is up to date", async () => {
    const { fake } = await launch();
    await pastCheckDelay();
    expect(fake.updateCalls).toEqual(["check"]);
    (await menuItem("Check for updates")).click();
    await vi.waitFor(() => {
      expect(toasts()).toContain("You're up to date.");
    });
    expect(fake.updateCalls).toEqual(["check", "check"]);
    // Rust hears which was the automatic one: no page runs that again this session.
    expect(fake.updateFlags).toEqual([true, false]);
    expect(pill()).toBeNull();
  });

  it("Check for updates shows what it found, or why it couldn't check", async () => {
    const { fake } = await launch((f) => void f.setSettings({ autoUpdate: false }));
    fake.update = INSTALLED;
    (await menuItem("Check for updates")).click();
    await vi.waitFor(() => {
      expect(pill()?.textContent).toBe("Update to v0.2.0");
    });
    expect(toasts()).toContain("Lectern 0.2.0 is available.");

    fake.updateError = "couldn't reach GitHub";
    (await menuItem("Check for updates")).click();
    await vi.waitFor(() => {
      expect(toasts()).toContain("Couldn't check for updates: couldn't reach GitHub");
    });
    // What was found before still stands.
    expect(pill()?.textContent).toBe("Update to v0.2.0");
  });

  it("asks before installing while another window holds an unsaved comment", async () => {
    const { fake } = await launch((f) => {
      f.update = INSTALLED;
      f.unsavedElsewhere = ["Personal"];
    });
    await pastCheckDelay();
    const confirm = (): Promise<HTMLElement> =>
      vi.waitFor(() => {
        const found = document.querySelector<HTMLElement>(".ws-confirm");
        if (!found) throw new Error("no confirm");
        return found;
      });
    pill()?.click();
    let asked = await confirm();
    expect(asked.querySelector("h2")?.textContent).toBe("Unsaved comment in Personal.");
    expect(asked.querySelector("p")?.textContent).toBe(
      "Lectern restarts to update, which discards it.",
    );
    expect([...asked.querySelectorAll(".btn")].map((b) => b.textContent)).toEqual([
      "Update anyway",
      "Cancel",
    ]);
    expect(document.activeElement?.textContent).toBe("Cancel");
    asked.querySelectorAll<HTMLButtonElement>(".btn")[1]?.click();
    await vi.waitFor(() => {
      expect(pill()?.textContent).toBe("Update to v0.2.0");
    });
    expect(fake.updateCalls).toEqual(["check", "install"]);
    pill()?.click();
    asked = await confirm();
    asked.querySelectorAll<HTMLButtonElement>(".btn")[0]?.click();
    await vi.waitFor(() => {
      expect(fake.updateCalls).toEqual(["check", "install", "install", "install"]);
    });
    // Unforced, unforced, then forced.
    expect(fake.updateFlags).toEqual([true, false, false, true]);
  });

  it("asks about this window's own unsaved comment first", async () => {
    const { fake } = await launch((f) => {
      f.update = INSTALLED;
    });
    await pastCheckDelay();
    const { CommentsController } = await import("../src/comments");
    const typed = vi.spyOn(CommentsController.prototype, "hasUnsavedText").mockReturnValue(true);
    pill()?.click();
    const asked = await vi.waitFor(() => {
      const found = document.querySelector<HTMLElement>(".ws-confirm");
      if (!found) throw new Error("no confirm");
      return found;
    });
    expect(asked.querySelector("h2")?.textContent).toBe("Your comment isn't saved");
    expect(asked.querySelector("p")?.textContent).toBe(
      "Updating Lectern discards what you've typed.",
    );
    expect([...asked.querySelectorAll(".btn")].map((b) => b.textContent)).toEqual([
      "Keep writing",
      "Update anyway",
    ]);
    asked.querySelectorAll<HTMLButtonElement>(".btn")[0]?.click();
    await settle(20);
    expect(fake.updateCalls).toEqual(["check"]);
    typed.mockRestore();
  });
});
