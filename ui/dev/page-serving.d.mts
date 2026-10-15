// Types for dev/page-serving.mjs, which the dev server runs as plain JavaScript.

/** The policies a page is served with. */
export interface PagePolicy {
  enforced: string;
  reportOnly: string | null;
}

export function pagePolicy(
  mode: string | null,
  nonce: string,
  token: string | null,
  reportOrigin: string,
): PagePolicy;

export function injectScript(html: string, script: string): string;

export const BRIDGE_KEY_SLOT: string;

export function fakeBridgeKey(token: string): string;

export function bridgeFor(bridge: string, key: string): string;
