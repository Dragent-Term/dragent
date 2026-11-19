import { parsePairLink } from "../src/pairing/pairing";
import { setPendingPairLink } from "../src/pairing/pendingLink";

/**
 * Deep-link entry for pairing (docs/design/e2ee.md §6.2 step 3). The full v2
 * link is parsed and stashed in memory; only a bare route enters
 * navigation — `d`, `k` and above all `p` never become route parameters
 * that navigation state or history keeps. Unparseable links land on the scan
 * screen instead.
 */
export function redirectSystemPath({ path }: { path: string; initial: boolean }): string {
  try {
    const url = new URL(path, "dragent://");
    if (url.protocol === "dragent:" && (url.hostname === "pair" || url.pathname.includes("pair"))) {
      try {
        setPendingPairLink(parsePairLink(url.toString(), __DEV__));
        return "/pair-confirm";
      } catch {
        return "/pair-scan";
      }
    }
    return "/";
  } catch { return "/"; }
}
