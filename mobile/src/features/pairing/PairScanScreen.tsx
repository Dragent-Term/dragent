import { useRef, useState } from "react";
import { StyleSheet, View } from "react-native";
import { CameraView, useCameraPermissions } from "expo-camera";
import { router } from "expo-router";
import { AppText, Button, FormField, Icon, PageHeader } from "../../components";
import { ScrollScreen } from "../../layout";
import { spacing, radii } from "../../theme";
import { useDevice } from "../../pairing/DeviceContext";
import { parsePairLink } from "../../pairing/pairing";
import { setPendingPairLink } from "../../pairing/pendingLink";
import { Feedback } from "../shared/Feedback";

/**
 * Pair-scan: camera for the desktop QR plus a "Paste the full link"
 * fallback. A successful scan or paste stashes the v2 link in memory and
 * lands on /pair-confirm with no route parameters; nothing is redeemed
 * here. Bare codes are refused (§6.2): without a camera, the person pastes
 * the full link shown under the QR.
 */
export function PairScanScreen() {
  const [permission, requestPermission] = useCameraPermissions();
  const { status } = useDevice();
  const scanned = useRef(false);
  const [code, setCode] = useState("");
  const [linkError, setLinkError] = useState<string | null>(null);

  const submitCode = () => {
    setLinkError(null);
    try {
      setPendingPairLink(parsePairLink(code, __DEV__));
      scanned.current = true;
      router.replace("/pair-confirm");
    } catch (error) {
      setLinkError(error instanceof Error ? error.message : "That code could not be read. Try again.");
    }
  };

  const onScanned = (data: string) => {
    if (scanned.current) return;
    try {
      setPendingPairLink(parsePairLink(data, __DEV__));
      scanned.current = true;
      router.replace("/pair-confirm");
    } catch {
      // Ignore non-pairing QR codes; the paste fallback explains the format.
    }
  };

  return <ScrollScreen maxWidth={680}>
    <PageHeader
      title={status === "paired" ? "Pair another phone" : "Pair your phone"}
      titleVariant="title"
      subtitle="On your desktop, open Settings → Mobile and scan the QR code."
    />
    {permission?.granted ? <View style={styles.cameraFrame}>
      <CameraView
        style={styles.camera}
        facing="back"
        barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
        onBarcodeScanned={({ data }) => onScanned(data)}
      />
    </View> : <View style={styles.permission}>
      <AppText tone="textMuted">Allow camera access to scan the pairing QR. You can also paste the link instead.</AppText>
      {permission?.canAskAgain !== false
        ? <Button label="Allow camera access" onPress={() => { void requestPermission(); }} /> : null}
    </View>}
    <View style={styles.paste}>
      <FormField
        label="Paste pairing link"
        value={code}
        onChangeText={(value) => { setCode(value); setLinkError(null); }}
        placeholder="dragent://pair?v=2&c=…"
        multiline
        autoCapitalize="none"
        autoCorrect={false}
        textContentType="none"
        importantForAccessibility="no"
        hint="Paste the full link shown under the QR if the camera cannot read it. A typed code alone can't pair this phone."
      />
      <Feedback message={linkError} />
      <View style={styles.actions}>
        <Button
          label="Continue"
          disabled={!code.trim()}
          trailing={<Icon name="arrow-right" size={16} tone="onAccent" />}
          onPress={submitCode}
        />
      </View>
    </View>
  </ScrollScreen>;
}

const styles = StyleSheet.create({
  cameraFrame: { height: 320, borderRadius: radii.lg, overflow: "hidden", marginVertical: spacing.md },
  camera: { flex: 1 },
  permission: { gap: spacing.md, marginVertical: spacing.md, alignItems: "flex-start" },
  paste: { gap: spacing.md, marginTop: spacing.md },
  actions: { alignItems: "flex-start" },
});
