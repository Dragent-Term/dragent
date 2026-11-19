/**
 * Mobile realtime-protocol constants.
 *
 * The canonical gateway protocol lives in auth-portal/src/lib/sessionSocket.ts
 * (`PROTOCOL_VERSION = 2` with the Wire* frame types). That version is mirrored
 * below as GATEWAY_PROTOCOL_VERSION — mirrored, never imported, so Metro only
 * ever bundles mobile/ sources.
 *
 * The current Go gateway waits for a join frame and replies with joined or
 * ended. It does not advertise a version or a minimum mobile version, so the
 * evaluator below is reserved for a future negotiated-version contract.
 */

/** Protocol version this app speaks. */
export const DRAGENT_MOBILE_PROTOCOL_VERSION = 2;

/** Gateway protocol version (mirror of auth-portal sessionSocket PROTOCOL_VERSION). */
export const GATEWAY_PROTOCOL_VERSION = 2;

export type Compatibility = "compatible" | "upgrade-app" | "upgrade-gateway";

export interface CompatibilityReport {
  verdict: Compatibility;
  mobile: number;
  gateway: number;
  message: string;
}

/**
 * Compare versions when server version metadata becomes available. Until a
 * compatibility range is negotiated, only equal versions are assumed safe.
 * This helper is not currently wired to the transport.
 */
export function evaluateCompatibility(
  gatewayVersion: number,
  mobileVersion: number = DRAGENT_MOBILE_PROTOCOL_VERSION,
): CompatibilityReport {
  if (gatewayVersion < mobileVersion) {
    return {
      verdict: "upgrade-gateway",
      mobile: mobileVersion,
      gateway: gatewayVersion,
      message: `Gateway speaks v${gatewayVersion} but this app speaks v${mobileVersion}. Update the gateway to join.`,
    };
  }
  if (gatewayVersion > mobileVersion) {
    return {
      verdict: "upgrade-app",
      mobile: mobileVersion,
      gateway: gatewayVersion,
      message: `Gateway speaks v${gatewayVersion} but this app speaks v${mobileVersion}. Update Dragent to join.`,
    };
  }
  return {
    verdict: "compatible",
    mobile: mobileVersion,
    gateway: gatewayVersion,
    message: `Negotiated protocol v${mobileVersion}.`,
  };
}

/** First frame sent as soon as the socket opens. */
export interface MobileJoinFrame {
  type: "join";
  protocol_version: number;
  session: string;
  secret: string;
  device_id: string;
  display_name: string;
  /** Additive v2 extension. Older gateways ignore it and send no boundary. */
  capabilities: { replay_boundary: true };
}

export function buildJoinFrame(input: {
  session: string;
  secret: string;
  deviceId: string;
  displayName: string;
}): MobileJoinFrame {
  return {
    type: "join",
    protocol_version: DRAGENT_MOBILE_PROTOCOL_VERSION,
    session: input.session,
    secret: input.secret,
    device_id: input.deviceId,
    display_name: input.displayName,
    capabilities: { replay_boundary: true },
  };
}

// Generated declarations share the canonical contract without pulling browser
// implementation code into Metro or mobile's stricter TypeScript project.
export type {
  Participant,
  Role,
  TerminalDelta,
  TerminalSurfaceState,
  WireAgentBlock,
  WireColor,
  WireLine,
  WireSpan,
} from "./wire-types.generated";
