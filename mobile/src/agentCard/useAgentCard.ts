/**
 * Keeps the live agent card in step with the desktops' agents: starts it
 * when an agent is working or waiting, updates it as they change, ends it
 * when all is quiet. iOS: a Live Activity. Android: an ongoing notification.
 *
 * End-to-end encryption: the card is drawn on the phone from what the
 * desktops send it encrypted; nothing about it goes through the server. It
 * updates while the app runs (and for a moment after it's put away); when
 * the app is suspended the card keeps its last state, marked as old after a
 * while, and the next push or open brings it up to date.
 */
import { useEffect } from "react";
import { AppState, Platform } from "react-native";
import { getCard, subscribeCard } from "./store";
import { stateLabel, type CardContent } from "./model";
import type { AgentCardProps } from "./props";

/** Quiet changes are gathered for this long before the card is redrawn. */
const SETTLE_MS = 1200;
/** iOS dims a card this long after its last update. */
const STALE_MS = 30 * 60 * 1000;
/**
 * Once every agent stops, the card says "All done" for this long, and work
 * that starts again in that time brings the same card back. Then it is
 * removed at once. (Ended the default way, iOS keeps a card on the Lock
 * Screen for up to four hours where the app can no longer reach it, so each
 * new session stacked another card.)
 */
const QUIET_MS = 10 * 60 * 1000;
let quietSince: number | null = null;

const DONE: AgentCardProps = { summary: "All done", needsYou: 0, working: 0, rows: [] };
const ANDROID_ID = "dragent-agent-card";

function propsOf(card: CardContent): AgentCardProps {
  return {
    summary: card.summary,
    needsYou: card.needsYou,
    working: card.working,
    rows: card.rows.map((row) => ({
      key: row.key,
      title: row.title,
      place: row.place,
      state: row.state,
      label: stateLabel(row.state),
      since: row.since,
    })),
  };
}

function linkOf(card: CardContent): string {
  const top = card.rows[0];
  return top ? `dragent://machine/${encodeURIComponent(top.desktopId)}` : "dragent://";
}

interface Layouts {
  AgentCardActivity: typeof import("./AgentCardActivity").AgentCardActivity;
  AgentCardNotification: typeof import("./AgentCardNotification").AgentCardNotification;
}

async function layouts(): Promise<Layouts | null> {
  try {
    // Each platform loads only its own layout.
    if (Platform.OS === "ios") {
      const { AgentCardActivity } = await import("./AgentCardActivity");
      return { AgentCardActivity } as Layouts;
    }
    const { AgentCardNotification } = await import("./AgentCardNotification");
    return { AgentCardNotification } as Layouts;
  } catch {
    // A build without the widgets module: no card, nothing breaks.
    return null;
  }
}

/** What to do next: `recheck` when an "All done" card is waiting to be removed. */
async function apply(card: CardContent | null): Promise<"recheck" | null> {
  const ui = await layouts();
  if (!ui) return null;
  if (Platform.OS === "ios") {
    const running = ui.AgentCardActivity.getInstances();
    if (!card) {
      if (running.length === 0) {
        quietSince = null;
        return null;
      }
      quietSince ??= Date.now();
      if (Date.now() - quietSince < QUIET_MS) {
        await running[0]!.update(DONE, new Date(Date.now() + STALE_MS)).catch(() => {});
        await Promise.all(running.slice(1).map((activity) => activity.end("immediate").catch(() => {})));
        return "recheck";
      }
      // Gone from the Lock Screen too, so no ended card lingers.
      await Promise.all(running.map((activity) => activity.end("immediate").catch(() => {})));
      quietSince = null;
      return null;
    }
    quietSince = null;
    const props = propsOf(card);
    const stale = new Date(Date.now() + STALE_MS);
    if (running.length > 0) {
      await running[0]!.update(props, stale);
      // One card only.
      await Promise.all(running.slice(1).map((activity) => activity.end("immediate").catch(() => {})));
    } else if (AppState.currentState === "active") {
      // iOS lets an app start a Live Activity only while it is in front.
      ui.AgentCardActivity.start(props, linkOf(card), stale);
    }
    return null;
  }
  if (Platform.OS === "android") {
    const shown = await ui.AgentCardNotification.get(ANDROID_ID).catch(() => null);
    if (!card) {
      await shown?.cancel().catch(() => {});
      return null;
    }
    const props = propsOf(card);
    const options = { title: card.rows[0]?.title ?? "Dragent", text: card.summary, url: linkOf(card), ongoing: true, alertOnlyOnce: true };
    if (shown) await shown.update(props, options);
    else await ui.AgentCardNotification.post(props, { id: ANDROID_ID, ...options });
  }
  return null;
}

export function useAgentCard(): void {
  useEffect(() => {
    if (Platform.OS !== "ios" && Platform.OS !== "android") return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let recheck: ReturnType<typeof setTimeout> | null = null;
    let running = Promise.resolve();
    const schedule = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        // In order: never two redraws at once.
        running = running
          .then(() => apply(getCard()))
          .then((next) => {
            if (recheck) clearTimeout(recheck);
            // Remove the "All done" card when its time is up (or on the
            // next open, if the app was asleep by then).
            recheck = next === "recheck" ? setTimeout(schedule, QUIET_MS + 1000) : null;
          })
          .catch(() => {});
      }, SETTLE_MS);
    };
    const unsubscribe = subscribeCard(schedule);
    // Back in front: an iOS card can only be started now, so try again.
    const app = AppState.addEventListener("change", (state) => {
      if (state === "active") schedule();
    });
    schedule();
    return () => {
      if (timer) clearTimeout(timer);
      if (recheck) clearTimeout(recheck);
      unsubscribe();
      app.remove();
    };
  }, []);
}
