import { useCallback, useEffect, useRef, useState } from "react";
import { useFocusEffect } from "expo-router";
import { useMachine } from "../../machines/MachineContext";
import { markTabSeen } from "../../transport/relayRpcMethods";

/**
 * While this tab's screen is up, the desktop hears that the person sees it:
 * once when the screen comes up, and again whenever the tab turns to
 * Review or Failed under their eyes. The desktop then shows it idle, here
 * and on its own screen, as if it had been opened there.
 */
export function useTabSeen(tabId: number) {
  const { rpc, catalog } = useMachine();
  const [focused, setFocused] = useState(false);
  useFocusEffect(
    useCallback(() => {
      setFocused(true);
      return () => setFocused(false);
    }, []),
  );
  const tab = catalog.status === "live" ? catalog.catalog.tabs.find((entry) => entry.id === tabId) ?? null : null;
  // What would make the desktop show news: a finished or failed turn.
  const news = tab !== null && (tab.agent_state === "review" || tab.attention === "failed") && tab.attention !== "needs_input";
  const state = tab ? `${tab.agent_state ?? ""}|${tab.attention ?? ""}` : null;
  const sent = useRef<string | null>(null);

  useEffect(() => {
    if (!focused) {
      sent.current = null;
      return;
    }
    if (!rpc || state === null) return;
    // On coming up, then on each new piece of news while up.
    if (sent.current !== null && (!news || sent.current === state)) return;
    sent.current = state;
    if (__DEV__) console.log(`[phone-seen] telling the desktop tab ${tabId} is seen (${state})`);
    markTabSeen(rpc, tabId).catch((error: unknown) => {
      if (__DEV__) console.log(`[phone-seen] tab ${tabId} not marked:`, error);
      // A desktop without `tabs.seen` (older build) or a dropped channel:
      // try again on the next change.
      sent.current = null;
    });
  }, [focused, rpc, tabId, state, news]);
}
