import { useCallback, useEffect, useRef, useState } from "react";
import { ExpoSpeechRecognitionModule, useSpeechRecognitionEvent } from "expo-speech-recognition";
import { dictationFailure, meterLevel, pushLevel } from "./dictationText";

export type DictationPhase =
  /** Not listening. */
  | "idle"
  /** Tapped: permission and the audio session are still starting. */
  | "preparing"
  | "recording"
  /** Stopped: the recogniser is flushing its last words. */
  | "finalizing";

/** How long a stop may take to flush before what was heard is kept as is. */
const FINALIZE_BACKSTOP_MS = 3000;

function deviceLocale(): string | undefined {
  try {
    return Intl.DateTimeFormat().resolvedOptions().locale || undefined;
  } catch {
    return undefined;
  }
}

/**
 * Speech-to-text for the message box. Each final recognition result is
 * handed to `onSettled` for the caller to append; the interim guess is
 * exposed as `heard` for display only. On an error or an unanswered stop,
 * the latest interim guess is kept rather than lost. Uses on-device
 * recognition when the phone supports it.
 */
export function useDictation(onSettled: (text: string) => void) {
  const [phase, setPhase] = useState<DictationPhase>("idle");
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [levels, setLevels] = useState<number[]>([]);
  const [heard, setHeard] = useState("");
  const [error, setError] = useState<string | null>(null);
  const heardRef = useRef("");
  const phaseRef = useRef<DictationPhase>("idle");
  const settle = useRef(onSettled);
  settle.current = onSettled;
  const backstop = useRef<ReturnType<typeof setTimeout> | null>(null);

  const move = useCallback((next: DictationPhase) => {
    phaseRef.current = next;
    setPhase(next);
  }, []);
  const keepHeard = useCallback(() => {
    if (heardRef.current.trim()) settle.current(heardRef.current);
    heardRef.current = "";
    setHeard("");
  }, []);
  const finish = useCallback(() => {
    if (backstop.current) clearTimeout(backstop.current);
    backstop.current = null;
    keepHeard();
    setLevels([]);
    setStartedAt(null);
    move("idle");
  }, [keepHeard, move]);

  useSpeechRecognitionEvent("start", () => {
    if (phaseRef.current === "idle") return;
    setStartedAt(Date.now());
    move("recording");
  });
  useSpeechRecognitionEvent("result", (event) => {
    if (phaseRef.current === "idle") return;
    const text = event.results[0]?.transcript ?? "";
    if (event.isFinal) {
      heardRef.current = "";
      setHeard("");
      if (text.trim()) settle.current(text);
    } else {
      heardRef.current = text;
      setHeard(text);
    }
  });
  useSpeechRecognitionEvent("volumechange", (event) => {
    if (phaseRef.current !== "recording") return;
    setLevels((current) => pushLevel(current, meterLevel(event.value)));
  });
  useSpeechRecognitionEvent("end", () => {
    if (phaseRef.current !== "idle") finish();
  });
  useSpeechRecognitionEvent("error", (event) => {
    if (phaseRef.current === "idle") return;
    setError(dictationFailure(event.error));
    finish();
  });

  const start = useCallback(async () => {
    if (phaseRef.current !== "idle") return;
    setError(null);
    move("preparing");
    try {
      const permission = await ExpoSpeechRecognitionModule.requestPermissionsAsync();
      if (!permission.granted) {
        setError(dictationFailure("not-allowed"));
        move("idle");
        return;
      }
      // Stopped while permission was being asked.
      if ((phaseRef.current as DictationPhase) !== "preparing") return;
      const lang = deviceLocale();
      ExpoSpeechRecognitionModule.start({
        ...(lang ? { lang } : {}),
        interimResults: true,
        continuous: true,
        addsPunctuation: true,
        // On this phone when it can: the audio never leaves it.
        requiresOnDeviceRecognition: ExpoSpeechRecognitionModule.supportsOnDeviceRecognition(),
        volumeChangeEventOptions: { enabled: true, intervalMillis: 100 },
      });
    } catch {
      setError(dictationFailure("audio-capture"));
      move("idle");
    }
  }, [move]);

  const stop = useCallback(() => {
    const now = phaseRef.current;
    if (now === "idle" || now === "finalizing") return;
    if (now === "preparing") {
      ExpoSpeechRecognitionModule.abort();
      finish();
      return;
    }
    move("finalizing");
    ExpoSpeechRecognitionModule.stop();
    // If the recogniser never answers the stop, keep what was heard and settle.
    backstop.current = setTimeout(() => {
      if (phaseRef.current !== "finalizing") return;
      ExpoSpeechRecognitionModule.abort();
      finish();
    }, FINALIZE_BACKSTOP_MS);
  }, [finish, move]);

  // Leaving the screen ends the recording.
  useEffect(() => () => {
    if (backstop.current) clearTimeout(backstop.current);
    if (phaseRef.current !== "idle") ExpoSpeechRecognitionModule.abort();
  }, []);

  return { phase, startedAt, levels, heard, error, start, stop, clearError: () => setError(null) };
}
