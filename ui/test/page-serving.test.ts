// The fake's dev server (dev/serve.mjs) serves pages as `lxpage` does: these check its policies,
// its bridge injection and what it lets a page read (dev/page-serving.mjs) against what Rust gives
// (src/generated/page-vectors.ts, written by `cargo test -p lectern-core`), so the e2e runs against a
// copy proven equal.
import { describe, expect, it } from "vitest";
import {
  BRIDGE_KEY_SLOT,
  bridgeFor,
  injectScript,
  pagePolicy,
  SHARED_EXTENSIONS,
  sharedWithPage,
} from "../dev/page-serving.mjs";
import {
  BRIDGE_KEY_SLOT as RUST_SLOT,
  INJECTIONS,
  PAGE_POLICIES,
  SHARED_EXTENSIONS as RUST_SHARED,
} from "../src/generated/page-vectors";

describe("the dev server's pages", () => {
  it("carry the policies Rust gives each mode, with an open to report to and without", () => {
    expect(PAGE_POLICIES).toHaveLength(6);
    for (const vector of PAGE_POLICIES) {
      const policy = pagePolicy(vector.mode, "NONCE", vector.token, "http://lxpage.localhost");
      expect(policy, `${vector.mode} ${String(vector.token)}`).toEqual({
        enforced: vector.enforced,
        reportOnly: vector.reportOnly,
      });
    }
  });

  it("put the bridge where Rust puts it, in every page made to trip it up", () => {
    expect(INJECTIONS.length).toBeGreaterThan(20);
    for (const { page, injected } of INJECTIONS) {
      expect(injectScript(page, '<script nonce="NONCE">BRIDGE</script>'), page).toBe(injected);
    }
  });

  it("put the open's key where Rust puts it in the bridge", () => {
    expect(BRIDGE_KEY_SLOT).toBe(RUST_SLOT);
    expect(bridgeFor(`(function (w, key) {})(window, ${RUST_SLOT});`, "k1")).toBe(
      '(function (w, key) {})(window, "k1");',
    );
  });

  it("let a page read only the files beside it that Rust lets it read, by extension", () => {
    expect(SHARED_EXTENSIONS).toEqual(RUST_SHARED);
    for (const ext of RUST_SHARED) {
      expect(sharedWithPage(`C:\\Fixtures\\vault\\lib\\a.${ext}`), ext).toBe(true);
      expect(sharedWithPage(`a.${ext.toUpperCase()}`), ext).toBe(true);
    }
    for (const name of [
      "a.json",
      "a.html",
      "a.htm",
      "a.pdf",
      "a.css",
      "a.png",
      "a.svg",
      "a.md",
      "mjs",
      "a.mjs.json",
    ]) {
      expect(sharedWithPage(name), name).toBe(false);
    }
  });
});
