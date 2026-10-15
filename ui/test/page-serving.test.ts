// The fake's dev server (dev/serve.mjs) serves pages as `lxpage` does: these check its policies and
// its bridge injection (dev/page-serving.mjs) against what Rust gives for the same input
// (src/generated/page-vectors.ts, written by `cargo test -p lectern-core`), so the e2e runs against a
// copy proven equal.
import { describe, expect, it } from "vitest";
import { BRIDGE_KEY_SLOT, bridgeFor, injectScript, pagePolicy } from "../dev/page-serving.mjs";
import {
  BRIDGE_KEY_SLOT as RUST_SLOT,
  INJECTIONS,
  PAGE_POLICIES,
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
});
