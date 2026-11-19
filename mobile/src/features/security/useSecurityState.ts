import { useEffect, useMemo, useState } from "react";
import type { DesktopLinkStatus } from "../../transport/e2ee/desktopLink";
import { securityStateFor, type SecurityScreenState } from "./securityStates";

/**
 * Phone §9 screen for one desktop, with the key-change latch: a desktop
 * that previously held an open channel and now fails handshakes shows the
 * "security key changed" screen, while one that never connected shows
 * "can't connect". The latch is per mount (per desktop route).
 */
export function useSecurityState(input: {
  secure: DesktopLinkStatus | null | undefined;
  socketError: string | null | undefined;
  desktopLegacy?: boolean | undefined;
  desktopName?: string | undefined;
}): SecurityScreenState | null {
  const [wasOpen, setWasOpen] = useState(false);
  const openNow = input.secure?.state === "open";
  useEffect(() => {
    if (openNow) setWasOpen(true);
  }, [openNow]);
  return useMemo(
    () =>
      securityStateFor({
        secure: input.secure,
        socketError: input.socketError,
        wasOpen,
        desktopLegacy: input.desktopLegacy,
        desktopName: input.desktopName,
      }),
    [input.secure, input.socketError, wasOpen, input.desktopLegacy, input.desktopName],
  );
}
