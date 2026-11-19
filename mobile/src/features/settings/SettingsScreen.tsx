import { useEffect, useState } from "react";
import { StyleSheet, View, useWindowDimensions } from "react-native";
import { router } from "expo-router";
import Constants from "expo-constants";
import { AppText, Button, Control, FormField, Icon, PageHeader } from "../../components";
import { ScrollScreen, useResponsiveLayout } from "../../layout";
import { useTheme, type AppTheme } from "../../theme";
import { useDevice } from "../../pairing/DeviceContext";
import { revokePairedPhone } from "../../pairing/api";
import { getPairedDesktopLabel, normalizePairedDesktopLabel, setPairedDesktopLabel } from "../../pairing/pairedDesktops";
import { endCompanionConnections } from "../../machines/companionConnections";
import { phoneSecurity, unpairDesktop } from "../../transport/e2ee/native";
import { removeDesktop } from "./removeDesktop";
import { checkForNewVersion, openUpdate, thisBuild, useVersionState } from "../../appVersion/useVersionNotice";
import { usePush } from "../../notifications/PushProvider";
import { useAsyncAction } from "../../hooks/useAsyncAction";
import { Feedback } from "../shared/Feedback";
import { SecurityPreview } from "../security/SecurityPreview";

const SETTINGS_WIDTH = 1040;

function ThemePreview({ palette }: { palette: AppTheme }) {
  return <View accessible={false} aria-hidden importantForAccessibility="no-hide-descendants"
    style={[styles.preview, { backgroundColor: palette.colors.background, borderColor: palette.colors.border }]}>
    <View style={[styles.previewSidebar, { backgroundColor: palette.colors.surface, borderColor: palette.colors.border }]}>
      <View style={[styles.previewLogo, { backgroundColor: palette.colors.text }]} />
      {[0, 1, 2].map((item) => <View key={item} style={[styles.previewNav, { backgroundColor: palette.colors.text, opacity: item === 0 ? 0.3 : 0.1 }]} />)}
    </View>
    <View style={styles.previewMain}>
      <View style={[styles.previewTitle, { backgroundColor: palette.colors.text }]} />
      <View style={[styles.previewLine, { backgroundColor: palette.colors.textMuted }]} />
      <View style={[styles.previewBlock, { backgroundColor: palette.colors.surfaceRaised, borderColor: palette.colors.border }]}>
        <View style={[styles.previewLine, { backgroundColor: palette.colors.textSubtle }]} />
        <View style={[styles.previewShortLine, { backgroundColor: palette.colors.textSubtle }]} />
      </View>
    </View>
  </View>;
}

function ThemeOption({ option, selected, busy, width, onSelect }: { option: AppTheme; selected: boolean; busy: boolean; width: number; onSelect: () => void }) {
  const { theme } = useTheme();
  return <Control accessibilityRole="radio" accessibilityLabel={option.name} accessibilityState={{ checked: selected, disabled: busy }}
    disabled={busy} onPress={onSelect}
    style={({ pressed, hovered }) => [styles.option, { width, borderColor: selected ? theme.colors.text : theme.colors.border,
      backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : theme.colors.surface }]}>
    <ThemePreview palette={option} />
    <View style={styles.optionLabel}>
      <AppText variant="caption" style={styles.semibold}>{option.name}</AppText>
      <View style={[styles.radio, { borderColor: selected ? theme.colors.text : theme.colors.border,
        backgroundColor: selected ? theme.colors.accent : theme.colors.surface }]}>
        {selected ? <Icon name="check" size={11} tone="onAccent" /> : null}
      </View>
    </View>
  </Control>;
}

function gatewayHost(gateway: string): string {
  try {
    return new URL(gateway).host;
  } catch {
    return gateway;
  }
}

function PairedDesktopSection() {
  const { theme } = useTheme();
  const device = useDevice();
  const paired = device.device;
  const [label, setLabel] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!paired) return;
    let active = true;
    void getPairedDesktopLabel(paired.gateway).then((saved) => {
      if (active) {
        setLabel(saved);
        setDraft(saved ?? paired.displayName);
      }
    });
    return () => { active = false; };
  }, [paired]);

  const rename = useAsyncAction(async (next: string) => {
    if (!paired) return;
    const clean = normalizePairedDesktopLabel(next);
    await setPairedDesktopLabel(paired.gateway, clean);
    setLabel(clean);
    setEditing(false);
  });

  const forget = useAsyncAction(async () => {
    if (!paired) return;
    const gateway = device.gateway ?? paired.gateway;
    const userId = paired.userId;
    const deviceId = paired.deviceId;
    // §6.4: E2EE teardown first (local, total, immediate), the server last
    // and only for availability. Local state clears even if all of it fails.
    await removeDesktop({
      pairedDesktops: () => phoneSecurity().pairedDesktops(),
      unpairDesktop: (desktop) => unpairDesktop(desktop),
      revokePhoneDevice: async () => {
        const token = await device.getAccessToken().catch(() => null);
        if (token) {
          await revokePairedPhone(fetch, gateway, token, deviceId);
        }
      },
      endCompanionConnections: () => endCompanionConnections(userId),
      clearLabel: () => setPairedDesktopLabel(paired.gateway, null).then(() => undefined),
      forgetPairing: () => device.forget(),
    });
    router.replace("/");
  });

  if (!paired) {
    return <View style={styles.section}>
      <View style={styles.sectionHeading}>
        <AppText style={styles.sectionTitle}>Paired desktop</AppText>
        <AppText variant="caption" tone="textMuted">This phone is not paired yet.</AppText>
      </View>
      <Button label="Pair your phone" leading={<Icon name="scan" size={16} tone="onAccent" />} onPress={() => router.push("/pair-scan")} />
    </View>;
  }

  return <View style={styles.section}>
    <View style={styles.sectionHeading}>
      <AppText style={styles.sectionTitle}>Paired desktop</AppText>
      <AppText variant="caption" tone="textMuted">
        Paired as {paired.deviceName} · {gatewayHost(device.gateway ?? paired.gateway)}
      </AppText>
    </View>
    {editing ? <View style={styles.rename}>
      <FormField label="Desktop name" value={draft} onChangeText={setDraft} maxLength={100} autoCapitalize="words" />
      <Feedback message={rename.error} />
      <View style={styles.renameActions}>
        <Button label="Save name" loading={rename.pending} disabled={!draft.trim()}
          onPress={() => { void rename.run(draft); }} />
        <Button label="Cancel" variant="ghost" onPress={() => {
          setDraft(label ?? paired.displayName);
          setEditing(false);
        }} />
      </View>
    </View> : <View style={[styles.pairedRow, { borderColor: theme.colors.border }]}>
      <View style={styles.pairedCopy}>
        <AppText style={styles.semibold}>{label ?? paired.displayName}</AppText>
        <AppText variant="caption" tone="textMuted">{paired.deviceName} · connected</AppText>
      </View>
      <View style={styles.pairedActions}>
        <Button label="Rename" size="sm" variant="ghost" onPress={() => setEditing(true)} />
        <Button label="Forget" size="sm" variant="ghost" loading={forget.pending} onPress={() => { void forget.run(); }} />
      </View>
    </View>}
    <Feedback message={forget.error} />
  </View>;
}

function NotificationsSection() {
  const push = usePush();
  const enable = useAsyncAction(push.enable);
  const disable = useAsyncAction(push.disable);
  const pending = push.pending || enable.pending || disable.pending;
  const detail = push.status === "enabled"
    ? "This phone is registered for push. Foreground visits renew the lease."
    : push.status === "enabling"
      ? "Enabling push on this phone…"
      : push.status === "unavailable"
        ? (push.reason ?? "Push notifications are unavailable on this phone.")
        : (push.reason ?? "Get notified when your desktop needs you.");
  return <View style={styles.section}>
    <View style={styles.sectionHeading}>
      <AppText style={styles.sectionTitle}>Notifications</AppText>
      <AppText variant="caption" tone="textMuted">{detail}</AppText>
    </View>
    {push.status === "enabled" ? (
      <Button label="Disable push" variant="ghost" loading={pending} onPress={() => void disable.run()} />
    ) : (
      <Button label={push.status === "error" ? "Retry" : "Enable push"} disabled={push.status === "unavailable"} loading={pending} onPress={() => void enable.run()} />
    )}
    <Feedback message={enable.error ?? disable.error} />
  </View>;
}

function TroubleshootingSection() {
  return <View style={styles.section}>
    <View style={styles.sectionHeading}>
      <AppText style={styles.sectionTitle}>Troubleshooting</AppText>
      <AppText variant="caption" tone="textMuted">What happened to this phone's connections, and what to try when a desktop won't connect.</AppText>
    </View>
    <Button label="Connection log" variant="ghost" onPress={() => router.push("/connection-log")} />
  </View>;
}

function UpdatesSection() {
  const version = useVersionState();
  const build = thisBuild();
  const notice = version.notice;
  const detail = version.checking
    ? "Looking for a new version…"
    : notice.kind === "required"
      ? "This version can no longer work with your desktop. Update to continue."
      : notice.kind === "available"
        ? (notice.release.notes ?? "A new version is available.")
        : version.failed
          ? "Couldn't check for a new version. Check your connection."
          : version.checkedAt
            ? "You have the latest version."
            : "New versions come from TestFlight or the download page.";
  return <View style={styles.section}>
    <View style={styles.sectionHeading}>
      <AppText style={styles.sectionTitle}>Updates</AppText>
      <AppText variant="caption" tone="textMuted">{detail}</AppText>
      <AppText variant="caption" tone="textSubtle">{`Version ${Constants.expoConfig?.version ?? ""}${build !== null ? ` (${build})` : ""}`}</AppText>
    </View>
    {notice.kind !== "current" ? (
      <Button label="Update" onPress={() => void openUpdate(notice.release.url)} />
    ) : (
      <Button label="Check for updates" variant="ghost" loading={version.checking} disabled={version.checking} onPress={() => void checkForNewVersion(true)} />
    )}
  </View>;
}

export function SettingsScreen() {
  const { themes, selection, setThemeId, error } = useTheme();
  const layout = useResponsiveLayout();
  const { fontScale } = useWindowDimensions();
  const available = Math.min(SETTINGS_WIDTH, layout.maxContentWidth);
  const minimum = 146 * Math.max(1, fontScale);
  const columns = layout.size === "expanded" && available >= minimum * 4 + 36 ? 4 : available >= minimum * 2 + 12 ? 2 : 1;
  const optionWidth = Math.max(0, (available - (columns - 1) * 12) / columns);
  const select = useAsyncAction(setThemeId);
  const order = ["light", "dark", "gruvbox_light", "gruvbox_dark"];
  const options = [...themes].filter((option) => order.includes(option.id)).sort((a, b) => order.indexOf(a.id) - order.indexOf(b.id));
  return <ScrollScreen maxWidth={SETTINGS_WIDTH} contentStyle={styles.page}>
    <PageHeader title="Settings" subtitle="Preferences for your mobile workspace." />
    <PairedDesktopSection />
    <NotificationsSection />
    <UpdatesSection />
    <TroubleshootingSection />
    <View style={styles.section}>
      <View style={styles.sectionHeading}><AppText style={styles.sectionTitle}>Appearance</AppText>
        <AppText variant="caption" tone="textMuted">Choose how Dragent looks on this phone.</AppText></View>
      <View accessibilityRole="radiogroup" accessibilityLabel="Application theme" style={styles.themeGrid}>
        {options.map((option) => <ThemeOption key={option.id} option={option} width={optionWidth} selected={selection === option.id}
          busy={select.pending} onSelect={() => { void select.run(option.id); }} />)}
      </View>
      <Feedback message={select.error ?? error} />
    </View>
    {__DEV__ ? <View style={styles.section}>
      <View style={styles.sectionHeading}><AppText style={styles.sectionTitle}>Developer</AppText>
        <AppText variant="caption" tone="textSubtle">Sample screens for development builds only.</AppText></View>
      <Button label="Open preview" variant="ghost" onPress={() => router.push("/preview")} />
      <View style={styles.sectionHeading}><AppText style={styles.sectionTitle}>Security states (E2EE §9)</AppText>
        <AppText variant="caption" tone="textSubtle">Every phone-side failure screen with its trigger. Dev only.</AppText></View>
      <SecurityPreview onAction={() => {}} />
    </View> : null}
    <View style={styles.footer}><AppText variant="caption" tone="textSubtle">Dragent Mobile</AppText>
      <AppText variant="caption" tone="textSubtle">Version {Constants.expoConfig?.version ?? "—"}</AppText></View>
  </ScrollScreen>;
}

const styles = StyleSheet.create({
  page: { paddingTop: 12, gap: 36 }, section: { gap: 18 }, sectionHeading: { gap: 6 },
  sectionTitle: { fontSize: 15, fontWeight: "600" }, semibold: { fontWeight: "600" },
  themeGrid: { flexDirection: "row", flexWrap: "wrap", gap: 12 },
  option: { minWidth: 0, padding: 8, borderRadius: 9, borderWidth: 1, gap: 12 },
  optionLabel: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 8, paddingHorizontal: 3, paddingBottom: 4 },
  radio: { width: 17, height: 17, borderRadius: 9, borderWidth: 1, alignItems: "center", justifyContent: "center" },
  preview: { height: 96, borderRadius: 5, overflow: "hidden", borderWidth: 1, flexDirection: "row" },
  previewSidebar: { width: "23%", paddingHorizontal: 7, paddingVertical: 10, gap: 8, borderRightWidth: 1 },
  previewLogo: { width: 8, height: 8, borderRadius: 2, marginBottom: 5, opacity: 0.6 }, previewNav: { height: 3, borderRadius: 2, width: "85%" },
  previewMain: { flex: 1, paddingHorizontal: 12, paddingTop: 14, gap: 8 },
  previewTitle: { height: 4, width: "47%", borderRadius: 2, opacity: 0.65 },
  previewLine: { height: 3, width: "75%", borderRadius: 2, opacity: 0.3 }, previewShortLine: { height: 3, width: "44%", borderRadius: 2, opacity: 0.3 },
  previewBlock: { marginTop: 3, borderWidth: 1, borderRadius: 3, padding: 7, gap: 5, height: 29 },
  pairedRow: { flexDirection: "row", alignItems: "center", gap: 14, borderWidth: 1, borderRadius: 9, padding: 14 },
  pairedCopy: { flex: 1, minWidth: 0, gap: 5 },
  pairedActions: { flexDirection: "row", gap: 8 },
  rename: { gap: 12 },
  renameActions: { flexDirection: "row", flexWrap: "wrap", gap: 12 },
  footer: { flexDirection: "row", flexWrap: "wrap", gap: 12, justifyContent: "space-between", paddingBottom: 12 },
});
