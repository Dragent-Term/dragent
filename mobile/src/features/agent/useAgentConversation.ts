import { useCallback, useEffect, useRef, useState } from "react";
import { useMachine } from "../../machines/MachineContext";
import {
  applyAgentUpdate,
  EMPTY_AGENT_CONVERSATION,
  type AgentConversationState,
} from "../../transport/agentConversation";
import {
  actOnAgent,
  sendAgentPrompt,
  subscribeAgentConversation,
  type AgentAction,
} from "../../transport/relayRpcMethods";

export type AgentDelivery = { readonly ok: true } | { readonly ok: false; readonly message: string };

export interface UseAgentConversation {
  readonly state: AgentConversationState;
  /**
   * `connecting` until the first message; `live` while streaming;
   * `unsupported` when this desktop build has no agent conversation to give
   * (the caller falls back to what it showed before); `lost` otherwise.
   */
  readonly phase: "connecting" | "live" | "unsupported" | "lost";
  readonly error: string | null;
  readonly send: (text: string) => Promise<AgentDelivery>;
  readonly act: (action: AgentAction, extra?: { ref?: string | null; option?: string; text?: string }) => Promise<AgentDelivery>;
  readonly retry: () => void;
}

const NOT_CONNECTED = "Not connected to your desktop. Nothing was sent.";
const UNCONFIRMED = "Couldn't confirm that reached your desktop. Check the conversation before sending again.";
/** Automatic resubscribes after a lost stream, before asking the person. */
const MAX_AUTO_RETRIES = 3;
/** A stream that sends nothing this long after subscribing is tried again. */
const FIRST_MESSAGE_MS = 15000;

function messageOf(error: unknown): string {
  const text = (error as { message?: unknown } | null)?.message;
  return typeof text === "string" && text !== "" ? text : UNCONFIRMED;
}

/**
 * The built-in agent's live conversation for one tab, over the machine's
 * encrypted channel. Resubscribes when the connection is replaced; a message
 * that names rows this phone doesn't hold asks for everything again.
 */
export function useAgentConversation(tabId: number): UseAgentConversation {
  const { rpc, socket } = useMachine();
  const [state, setState] = useState<AgentConversationState>(EMPTY_AGENT_CONVERSATION);
  const [phase, setPhase] = useState<UseAgentConversation["phase"]>("connecting");
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const autoRetries = useRef(0);
  const ready = rpc !== null && socket.status === "ready";

  useEffect(() => {
    if (!rpc || !ready) return;
    let active = true;
    let held = EMPTY_AGENT_CONVERSATION;
    setPhase((current) => (current === "live" ? current : "connecting"));
    const again = () => {
      if (!active) return;
      if (autoRetries.current >= MAX_AUTO_RETRIES) {
        setPhase("lost");
        return;
      }
      autoRetries.current += 1;
      setTimeout(() => { if (active) setAttempt((value) => value + 1); }, 400 * autoRetries.current);
    };
    // Subscribed on a connection that died unnoticed, nothing would ever
    // arrive and the screen would wait forever: retry instead.
    let heard = false;
    const silence = setTimeout(() => {
      if (!active || heard) return;
      stream.close();
      setError("Your desktop didn't answer. Trying again.");
      again();
    }, FIRST_MESSAGE_MS);
    const stream = subscribeAgentConversation(rpc, tabId, {
      onMessage: (payload, info) => {
        if (!active) return;
        heard = true;
        const result = applyAgentUpdate(held, payload, info.snapshot);
        if (!result.ok) {
          stream.close();
          again();
          return;
        }
        held = result.state;
        autoRetries.current = 0;
        setState(held);
        setError(null);
        setPhase("live");
      },
      onError: (failure) => {
        if (!active) return;
        heard = true;
        // An older desktop doesn't know the stream ("invalid"); a tab that
        // isn't the built-in agent's has none to give ("unavailable").
        if (failure.code === "invalid" || failure.code === "unavailable") {
          setPhase("unsupported");
          return;
        }
        setError(messageOf(failure));
        again();
      },
    });
    return () => {
      active = false;
      clearTimeout(silence);
      stream.close();
    };
  }, [rpc, ready, socket.connectionEpoch, tabId, attempt]);

  const send = useCallback(async (text: string): Promise<AgentDelivery> => {
    if (!rpc || !ready) return { ok: false, message: NOT_CONNECTED };
    try {
      await sendAgentPrompt(rpc, tabId, text);
      return { ok: true };
    } catch (failure) {
      return { ok: false, message: messageOf(failure) };
    }
  }, [rpc, ready, tabId]);

  const act = useCallback<UseAgentConversation["act"]>(async (action, extra) => {
    if (!rpc || !ready) return { ok: false, message: NOT_CONNECTED };
    try {
      await actOnAgent(rpc, tabId, action, extra);
      return { ok: true };
    } catch (failure) {
      return { ok: false, message: messageOf(failure) };
    }
  }, [rpc, ready, tabId]);

  const retry = useCallback(() => {
    autoRetries.current = 0;
    setError(null);
    setPhase("connecting");
    setAttempt((value) => value + 1);
  }, []);

  return { state, phase, error, send, act, retry };
}
