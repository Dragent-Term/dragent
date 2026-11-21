import { useEffect, useState } from "react";
import { Platform, StyleSheet, View } from "react-native";
import Constants from "expo-constants";
import { router } from "expo-router";
import { AppText, Button, Icon, PageHeader } from "../../components";
import { ScrollScreen } from "../../layout";
import { useDevice } from "../../pairing/DeviceContext";
import { PairingError } from "../../pairing/api";
import { isTrustedGateway } from "../../pairing/pairing";
import { clearPendingPairLink, peekPendingPairLink, takePendingPairLink } from "../../pairing/pendingLink";
import { useAsyncAction } from "../../hooks/useAsyncAction";
import { Feedback } from "../shared/Feedback";
import { logConnection } from "../../diagnostics/connectionLog";

/**
 * Pair-confirm: name this phone, then redeem and run the IKpsk1 handshake to
 * the desktop key from the scanned QR (docs/design/e2ee.md §6.2). The pending
 * link lives in memory only — never in route parameters — and its secret is
 * wiped when pairing finishes or is abandoned. The name travels inside the
 * encrypted handshake payload, never to the server.
 *
 * Error states are explicit: used/expired codes ask for a fresh desktop
 * code, a missing passcode asks for one (§9), rate-limited asks for
 * patience, offline asks for connectivity.
 */
/** This phone's own name (Settings → General → About → Name), or a plain default. */
export function phoneDisplayName(): string {
  const own = typeof Constants.deviceName === "string" ? Constants.deviceName.trim() : "";
  if (own) return own.slice(0, 64);
  return Platform.OS === "android" ? "Android phone" : "iPhone";
}

function hostOf(origin: string): string {
  try {
    return new URL(origin).host;
  } catch {
    return origin;
  }
}

export function PairConfirmScreen() {
  // The link this screen was opened with, for what it shows. Connecting takes
  // the stored link (and its secret) away, so reading it again on every redraw
  // turned an in-progress or finished pairing into "could not be used" and
  // hid any real error.
  const [pending] = useState(peekPendingPairLink);
  const device = useDevice();
  // No typing: the phone's own name is used, like any other paired device.
  const [name] = useState(phoneDisplayName);
  const [expired, setExpired] = useState(false);

  // Leaving without pairing abandons it: the secret is wiped (§6.2 step 3).
  useEffect(() => () => clearPendingPairLink(), []);

  const connect = useAsyncAction(async () => {
    setExpired(false);
    const link = takePendingPairLink();
    if (!link) {
      throw new Error("This pairing has expired. Scan the QR shown in Settings → Mobile on your desktop.");
    }
    try {
      await device.pair({ link, deviceName: name });
    } catch (error) {
      if (error instanceof PairingError && error.kind === "gone") setExpired(true);
      logConnection("error", null, `Pairing failed: ${error instanceof Error ? error.message : String(error)}`);
      throw error;
    }
    logConnection("info", null, `Paired with the desktop as “${name}”`);
    router.replace("/");
  });

  // Scanning is the consent for Dragent's own servers: connect straight away.
  // A link can also arrive from a web page or a message, so one that names
  // another server waits for the person to see which server and tap Connect.
  const trusted = pending ? isTrustedGateway(pending.gateway, __DEV__) : false;
  const [started, setStarted] = useState(false);
  useEffect(() => {
    if (pending && trusted && !started) {
      setStarted(true);
      void connect.run();
    }
  }, [pending, trusted, started, connect]);

  if (!pending) {
    return <ScrollScreen maxWidth={680}>
      <PageHeader title="Pair your phone" subtitle="This pairing could not be used." />
      <Feedback message="Scan the QR shown in Settings → Mobile on your desktop, then continue here." />
      <Button label="Scan again" leading={<Icon name="scan" size={16} tone="onAccent" />} onPress={() => router.replace("/pair-scan")} />
    </ScrollScreen>;
  }

  return <ScrollScreen maxWidth={680} contentStyle={styles.page}>
    <PageHeader
      title="Connect to your desktop"
      subtitle={expired
        ? "That code has already been used or has expired."
        : "Setting up a private, end-to-end encrypted connection."}
    />
    <View style={styles.form}>
      <AppText variant="caption" tone="textSubtle">Pairing code {pending.code.slice(0, 4)}…{pending.code.slice(-4)}</AppText>
      {!trusted && !connect.pending && !connect.error ? <>
        <AppText tone="text">This code connects to {hostOf(pending.gateway)}, which isn&apos;t a Dragent server.</AppText>
        <AppText tone="textMuted" style={styles.hint}>
          Only connect if you run that server yourself or trust whoever does: it would relay this phone&apos;s connection. If you didn&apos;t expect this, go back.
        </AppText>
      </> : <AppText tone="textMuted">{connect.pending ? `Connecting as “${name}”…` : connect.error ? "" : `Connecting as “${name}”…`}</AppText>}
      <Feedback message={connect.error} />
      {connect.error ? <AppText tone="textMuted" style={styles.hint}>
        On your desktop, open Settings → Mobile and pair again for a fresh code.
      </AppText> : null}
      <View style={styles.actions}>
        {/* A code works once: after a failed attempt only a new code can
            pair, so the button scans again rather than retrying (which
            always ended in "This pairing has expired" and hid the real error). */}
        {connect.error ? <Button
          label="Scan a new code"
          trailing={<Icon name="scan" size={16} tone="onAccent" />}
          onPress={() => router.replace("/pair-scan")}
        /> : <Button
          label="Connect"
          loading={connect.pending}
          disabled={connect.pending}
          trailing={<Icon name="arrow-right" size={16} tone="onAccent" />}
          onPress={() => { setStarted(true); void connect.run(); }}
        />}
        {!trusted && !connect.pending && !connect.error ? <Button
          label="Go back"
          variant="ghost"
          onPress={() => router.replace("/")}
        /> : null}
      </View>
    </View>
  </ScrollScreen>;
}

const styles = StyleSheet.create({
  page: { paddingTop: 12, gap: 32 },
  form: { gap: 22 },
  hint: { maxWidth: 480, lineHeight: 23 },
  actions: { alignItems: "flex-start", gap: 12 },
});
