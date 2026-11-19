/**
 * Two-way voice with an agent: listen, send what was said when you pause,
 * wait for the agent's reply, read it aloud, then listen again — until you
 * end it. Recognition and speech run on the phone (on-device recognition
 * where the phone has it); nothing but the text you'd type is sent.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { ExpoSpeechRecognitionModule, useSpeechRecognitionEvent } from "expo-speech-recognition";
import * as Speech from "expo-speech";
import { spokenText } from "../../chat/voice";
import { meterLevel } from "./dictationText";

export type VoicePhase = "off" | "starting" | "listening" | "sending" | "waiting" | "speaking" | "error";

/** A pause this long after words ends your turn. */
const PAUSE_MS = 1500;
/** The reply is read once it has stopped changing for this long. */
const REPLY_SETTLE_MS = 900;

export interface VoiceReply {
  readonly id: string;
  readonly text: string;
}

export function useVoiceConversation({
  send,
  reply,
  busy,
}: {
  /** Send what was said; false when it didn't go. */
  send: (text: string) => Promise<boolean>;
  /** The agent's latest finished reply. */
  reply: VoiceReply | null;
  /** The agent is still working on it. */
  busy: boolean;
}) {
  const [phase, setPhase] = useState<VoicePhase>("off");
  const [heard, setHeard] = useState("");
  const [level, setLevel] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const phaseRef = useRef<VoicePhase>("off");
  const heardRef = useRef("");
  const changedAt = useRef(0);
  const replyBefore = useRef<string | null>(null);
  const sendRef = useRef(send);
  sendRef.current = send;
  const latestReply = useRef(reply);
  latestReply.current = reply;

  const move = useCallback((next: VoicePhase) => {
    phaseRef.current = next;
    setPhase(next);
  }, []);

  const listen = useCallback(async () => {
    if (phaseRef.current === "off") return;
    heardRef.current = "";
    setHeard("");
    move("starting");
    try {
      const permission = await ExpoSpeechRecognitionModule.requestPermissionsAsync();
      if (!permission.granted) {
        setError("Allow the microphone and speech recognition to talk to the agent.");
        move("error");
        return;
      }
      if (phaseRef.current !== "starting") return;
      ExpoSpeechRecognitionModule.start({
        interimResults: true,
        continuous: true,
        addsPunctuation: true,
        requiresOnDeviceRecognition: ExpoSpeechRecognitionModule.supportsOnDeviceRecognition(),
        volumeChangeEventOptions: { enabled: true, intervalMillis: 100 },
        // Play and record, out loud: the reply is read on the speaker (or
        // headphones), not the earpiece.
        iosCategory: {
          category: "playAndRecord",
          categoryOptions: ["defaultToSpeaker", "allowBluetooth"],
          mode: "default",
        },
      });
    } catch {
      setError("The microphone isn't available.");
      move("error");
    }
  }, [move]);

  const deliver = useCallback(
    async (text: string) => {
      const words = text.trim();
      if (!words) {
        void listen();
        return;
      }
      move("sending");
      replyBefore.current = latestReply.current?.id ?? null;
      const sent = await sendRef.current(words);
      if (phaseRef.current === "off") return;
      if (!sent) {
        setError("That didn't reach the agent. Try again.");
        void listen();
        return;
      }
      move("waiting");
    },
    [listen, move],
  );

  useSpeechRecognitionEvent("start", () => {
    if (phaseRef.current === "starting") {
      changedAt.current = Date.now();
      move("listening");
    }
  });
  useSpeechRecognitionEvent("result", (event) => {
    if (phaseRef.current !== "listening") return;
    const text = event.results[0]?.transcript ?? "";
    if (text !== heardRef.current) changedAt.current = Date.now();
    heardRef.current = text;
    setHeard(text);
    if (event.isFinal && text.trim()) {
      ExpoSpeechRecognitionModule.abort();
      void deliver(text);
    }
  });
  useSpeechRecognitionEvent("volumechange", (event) => {
    if (phaseRef.current === "listening") setLevel(meterLevel(event.value));
  });
  useSpeechRecognitionEvent("end", () => {
    // The recogniser stopped on its own (a long silence): send what it heard.
    if (phaseRef.current === "listening") void deliver(heardRef.current);
  });
  useSpeechRecognitionEvent("error", (event) => {
    if (phaseRef.current !== "listening" && phaseRef.current !== "starting") return;
    if (event.error === "no-speech") {
      void listen();
      return;
    }
    setError("Voice stopped. Tap to talk again.");
    move("error");
  });

  // Your turn ends when you pause.
  useEffect(() => {
    if (phase !== "listening") return;
    const timer = setInterval(() => {
      if (phaseRef.current !== "listening") return;
      if (heardRef.current.trim() && Date.now() - changedAt.current > PAUSE_MS) {
        const words = heardRef.current;
        ExpoSpeechRecognitionModule.abort();
        void deliver(words);
      }
    }, 250);
    return () => clearInterval(timer);
  }, [phase, deliver]);

  // The agent's answer, read once it has settled.
  useEffect(() => {
    if (phase !== "waiting" || busy || !reply || reply.id === replyBefore.current || !reply.text.trim()) return;
    const timer = setTimeout(() => {
      if (phaseRef.current !== "waiting") return;
      move("speaking");
      Speech.speak(spokenText(reply.text), {
        onDone: () => { if (phaseRef.current === "speaking") void listen(); },
        onStopped: () => {},
        onError: () => { if (phaseRef.current === "speaking") void listen(); },
      });
    }, REPLY_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [phase, busy, reply, listen, move]);

  const start = useCallback(() => {
    setError(null);
    phaseRef.current = "starting";
    void listen();
  }, [listen]);

  const end = useCallback(() => {
    move("off");
    setHeard("");
    setLevel(0);
    ExpoSpeechRecognitionModule.abort();
    void Speech.stop();
  }, [move]);

  /** While it speaks: stop and listen. While you talk: send now. */
  const next = useCallback(() => {
    if (phaseRef.current === "speaking") {
      void Speech.stop();
      void listen();
    } else if (phaseRef.current === "listening" && heardRef.current.trim()) {
      const words = heardRef.current;
      ExpoSpeechRecognitionModule.abort();
      void deliver(words);
    } else if (phaseRef.current === "error") {
      setError(null);
      void listen();
    }
  }, [deliver, listen]);

  useEffect(() => () => {
    if (phaseRef.current !== "off") {
      ExpoSpeechRecognitionModule.abort();
      void Speech.stop();
    }
  }, []);

  return { phase, heard, level, error, active: phase !== "off", start, end, next };
}
