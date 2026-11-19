/**
 * The arithmetic behind dictation in the message box. Pure, so it is tested
 * without the microphone.
 */

/** Bars in the level meter while recording. */
export const METER_BARS = 5;

/**
 * The draft with `heard` appended, separated by a space unless the draft is
 * empty or already ends in whitespace. Typed text is always kept.
 */
export function appendDictation(draft: string, heard: string): string {
  const text = heard.trim();
  if (text === "") return draft;
  if (draft === "" || /\s$/.test(draft)) return draft + text;
  return `${draft} ${text}`;
}

/** The recogniser's volume (-2 … 10, below 0 is silence) as a bar height 0 … 1. */
export function meterLevel(volume: number): number {
  if (!Number.isFinite(volume) || volume <= 0) return 0;
  return Math.min(1, volume / 10);
}

/** The meter's history with `level` added, newest last, `METER_BARS` long. */
export function pushLevel(levels: readonly number[], level: number): number[] {
  const next = [...levels, level];
  return next.slice(Math.max(0, next.length - METER_BARS));
}

/** "0:07", "1:23", "12:05": how long the recording has run. */
export function recordingTime(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** What to tell the person when dictation stopped on its own, or null to say nothing. */
export function dictationFailure(code: string): string | null {
  switch (code) {
    case "aborted":
    case "no-speech":
    case "speech-timeout":
      return null;
    case "not-allowed":
      return "Dictation needs the microphone and speech recognition. Turn them on for Dragent in Settings.";
    case "language-not-supported":
      return "Dictation isn't available in this language on this iPhone.";
    case "interrupted":
      return "Dictation stopped: something else took the microphone.";
    case "network":
      return "Dictation needs a connection for this language.";
    case "busy":
      return "The microphone is busy. Try again in a moment.";
    default:
      return "Dictation stopped. What was heard is kept.";
  }
}
