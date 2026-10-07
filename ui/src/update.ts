// Updates (spec §9): the automatic check, at most once a day and quiet unless it finds an update;
// the ⋯ menu's "Check for updates", which always checks and says what it found; and the header
// pill an update brings. Its click installs the update and restarts (an installed copy), once
// Lectern may leave (unsaved comment text, here or in another window, is asked about first), or
// opens the Releases page (a portable copy); Rust decides which. While an install runs, the pill
// stays disabled whatever a check finds. Rust hears which check was the automatic one, which then
// runs no more this session. Loaded on first use.
import { h } from "./dom";
import type { UpdateInfo } from "./generated/UpdateInfo";

export interface UpdateHost {
  /** `automatic`: the check after startup. */
  checkUpdate(automatic: boolean): Promise<UpdateInfo | null>;
  /**
   * Installs the update, asking first about unsaved comment text, or (`portable`) opens the
   * Releases page.
   */
  installUpdate(portable: boolean): Promise<void>;
  toast(message: string): void;
}

/** Where the time of the last check that got an answer is kept. */
export const LAST_CHECK_KEY = "lx-update-checked";
/** The automatic check runs at most this often. */
export const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;

export class Updater {
  private pill: HTMLButtonElement | null = null;
  private update: UpdateInfo | null = null;
  /** The check in flight, which a second request joins. */
  private checking: Promise<UpdateInfo | null> | null = null;
  /** An installed copy is downloading the update or starting the installer. */
  private installing = false;

  /** The pill goes first in `slot`, the header's actions. */
  constructor(
    private readonly slot: HTMLElement,
    private readonly host: UpdateHost,
    private readonly now: () => number = Date.now,
  ) {}

  /**
   * The check after startup: skipped within a day of the last answered one, and quiet unless it
   * finds an update. A failure (offline, no release yet) is only logged and retried next launch.
   */
  async checkAutomatically(): Promise<void> {
    const last = lastCheck();
    if (last !== null && this.now() - last < CHECK_INTERVAL_MS) {
      return;
    }
    try {
      await this.check(true);
    } catch (e) {
      console.warn("update check failed", e);
    }
  }

  /** "Check for updates": always checks, and says what it found. */
  async checkNow(): Promise<void> {
    let found: UpdateInfo | null;
    try {
      found = await this.check(false);
    } catch (e) {
      this.host.toast(`Couldn't check for updates: ${String(e)}`);
      return;
    }
    this.host.toast(
      found === null ? "You're up to date." : `Lectern ${found.version} is available.`,
    );
  }

  /** The update found, if any: its pill shows. A check joins one in flight. */
  private async check(automatic: boolean): Promise<UpdateInfo | null> {
    this.checking ??= this.host.checkUpdate(automatic).finally(() => {
      this.checking = null;
    });
    const found = await this.checking;
    rememberCheck(this.now());
    this.show(found);
    return found;
  }

  private show(update: UpdateInfo | null): void {
    if (this.installing) {
      // The install under way keeps its pill; what a check found meanwhile can wait.
      return;
    }
    this.update = update;
    if (update === null) {
      this.pill?.remove();
      this.pill = null;
      return;
    }
    if (!this.pill) {
      this.pill = h("button", { type: "button", id: "lx-update-pill", class: "update-pill" });
      this.pill.addEventListener("click", () => void this.install());
      this.slot.prepend(this.pill);
    }
    this.label(update);
  }

  private label(update: UpdateInfo): void {
    if (!this.pill) {
      return;
    }
    const version = `v${update.version}`;
    this.pill.disabled = false;
    this.pill.textContent = update.portable ? `Download ${version}` : `Update to ${version}`;
    this.pill.title = update.portable
      ? `Open the Releases page to download Lectern ${update.version}`
      : `Install Lectern ${update.version}; Lectern restarts when it's done`;
  }

  /** Installs the update (Lectern then exits and restarts) or opens the Releases page. */
  private async install(): Promise<void> {
    const update = this.update;
    if (!this.pill || !update || this.installing) {
      return;
    }
    if (!update.portable) {
      this.installing = true;
      this.pill.disabled = true;
      this.pill.textContent = "Installing…";
    }
    try {
      await this.host.installUpdate(update.portable);
    } catch (e) {
      this.host.toast(
        update.portable
          ? `Couldn't open the Releases page: ${String(e)}`
          : `Couldn't update: ${String(e)}`,
      );
    } finally {
      this.installing = false;
    }
    if (this.update === update) {
      this.label(update);
    }
  }
}

/** When the last answered check ran, if known. Storage may be unavailable. */
function lastCheck(): number | null {
  try {
    const value = Number(localStorage.getItem(LAST_CHECK_KEY));
    return Number.isFinite(value) && value > 0 ? value : null;
  } catch {
    return null;
  }
}

function rememberCheck(at: number): void {
  try {
    localStorage.setItem(LAST_CHECK_KEY, String(at));
  } catch {
    // Then the next launch checks again.
  }
}
