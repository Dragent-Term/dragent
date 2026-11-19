import { StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, Icon, LoadingState, PageHeader, Section, Surface } from "../../components";
import { Screen } from "../../layout";
import { spacing } from "../../theme/tokens";
import { useDevice } from "../../pairing/DeviceContext";
import { MachinesSection } from "../machines/MachinesSection";
import { ActionRow } from "./ActionRow";
import { ResumeCard } from "./ResumeCard";

/**
 * Home: the paired desktops (live projects and tabs) with quick access.
 * Unpaired phones see one setup card; the desktop stays signed in and the
 * phone only scans its pairing code.
 */
export function HomeScreen() {
  const { ready, status, device } = useDevice();
  const paired = status === "paired" && device !== null;

  return (
    <Screen maxWidth={1040} contentStyle={styles.content}>
      <View style={styles.header}>
        <PageHeader title="Welcome back" titleVariant="hero"
          actions={__DEV__ ? <Button label="Preview" size="sm" variant="ghost"
            leading={<Icon name="grid" size={16} />} onPress={() => router.push("/preview")} /> : undefined} />
      </View>
      {!ready ? <LoadingState label="Loading…" /> : !paired ? (
        <Surface>
          <View style={styles.setup}>
            <AppText style={styles.setupTitle}>Connect your desktop</AppText>
            <AppText variant="caption" tone="textMuted">
              On your desktop, open Settings → Mobile and choose Pair a phone, then scan its QR code or enter the code.
            </AppText>
            <View style={styles.setupActions}>
              <Button
                label="Scan QR code"
                leading={<Icon name="scan" size={16} tone="onAccent" />}
                onPress={() => router.push("/pair-scan")}
              />
            </View>
          </View>
        </Surface>
      ) : (
        <View style={styles.footer}>
          <ResumeCard />
          <MachinesSection />
          <Section title="Quick access">
            <ActionRow
              icon="scan"
              title="Scan QR code"
              detail="Pair with another desktop"
              onPress={() => router.push("/pair-scan")}
            />
          </Section>
        </View>
      )}
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { gap: spacing.md },
  header: { paddingTop: spacing.sm, marginBottom: spacing.md },
  footer: { gap: spacing.xl },
  setup: { gap: spacing.md, paddingVertical: spacing.xs },
  setupTitle: { fontWeight: "600" },
  setupActions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
});
