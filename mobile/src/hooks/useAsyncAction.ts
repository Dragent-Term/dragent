import { useCallback, useEffect, useRef, useState } from "react";

/** Shared form lifecycle: prevent double submission and ignore unmounted UI. */
export function useAsyncAction<Args extends unknown[], Result>(action: (...args: Args) => Promise<Result>) {
  const latest = useRef(action);
  latest.current = action;
  const mounted = useRef(true);
  const running = useRef(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const run = useCallback(async (...args: Args): Promise<Result | undefined> => {
    if (running.current) return undefined;
    running.current = true;
    setPending(true); setError(null);
    try { return await latest.current(...args); }
    catch (reason) {
      if (mounted.current) setError(reason instanceof Error ? reason.message : "Something went wrong. Try again.");
      return undefined;
    } finally {
      running.current = false;
      if (mounted.current) setPending(false);
    }
  }, []);
  return { run, pending, error, clearError: () => setError(null) };
}
