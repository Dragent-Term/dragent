/**
 * The phone chat's extras, under and above the message box: attach photos,
 * screenshots and files from the phone, reference the workspace's files with
 * `@`, and pick the agent's model, as the desktop chat does.
 */
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { ActivityIndicator, Image, ScrollView, StyleSheet, View, useWindowDimensions } from "react-native";
import * as DocumentPicker from "expo-document-picker";
import * as ImagePicker from "expo-image-picker";
import { File, FileMode } from "expo-file-system";
import { AppText, Control, Icon, SelectPopover } from "../../components";
import { useTheme } from "../../theme";
import { useMachine } from "../../machines/MachineContext";
import { toBase64 } from "../../transport/e2ee/bytes";
import {
  MAX_UPLOAD_BYTES,
  completeMention,
  findWorkspaceFiles,
  getChatModels,
  mentionAtEnd,
  modelKey,
  setChatModel,
  uploadFile,
  withAttachments,
  type ChatModels,
  type FoundFile,
} from "../../chat/extras";

export interface Attachment {
  readonly key: string;
  readonly name: string;
  readonly uri: string;
  readonly image: boolean;
  readonly progress: number;
  /** Where the desktop saved it, once sent. */
  readonly path: string | null;
  readonly error: string | null;
}

/**
 * State and actions for one chat's extras. `tabId` names the tab whose
 * model menu applies (none before there is a tab); `workspaceRoot` is where
 * `@` looks for files.
 */
export function useChatExtras({ tabId, workspaceRoot }: { tabId: number | null; workspaceRoot: string | null }) {
  const { rpc } = useMachine();
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [menuOpen, setMenuOpen] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const cancelled = useRef(new Set<string>());

  const patch = useCallback((key: string, change: Partial<Attachment>) => {
    setAttachments((current) => current.map((item) => (item.key === key ? { ...item, ...change } : item)));
  }, []);

  const send = useCallback(
    async (picked: { uri: string; name: string; size: number | null; image: boolean }[]) => {
      if (!rpc) {
        setNotice("Your desktop isn't connected.");
        return;
      }
      for (const file of picked) {
        const key = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
        let size = file.size ?? 0;
        let handle: ReturnType<File["open"]> | null = null;
        try {
          const source = new File(file.uri);
          size = size || source.size || 0;
          if (size <= 0) throw new Error("This file is empty.");
          if (size > MAX_UPLOAD_BYTES) throw new Error(`${file.name} is larger than 50 MB.`);
          setAttachments((current) => [...current, { key, name: file.name, uri: file.uri, image: file.image, progress: 0, path: null, error: null }]);
          handle = source.open(FileMode.ReadOnly);
          const open = handle;
          const saved = await uploadFile(
            rpc,
            { name: file.name, size },
            async (offset, length) => {
              open.offset = offset;
              return toBase64(open.readBytesSync(length));
            },
            (share) => patch(key, { progress: share }),
            () => cancelled.current.has(key),
          );
          if (!cancelled.current.has(key)) patch(key, { progress: 1, path: saved.path });
        } catch (error) {
          if (cancelled.current.has(key)) continue;
          const message = error instanceof Error && error.message ? error.message : "It didn't reach your desktop.";
          setAttachments((current) =>
            current.some((item) => item.key === key)
              ? current.map((item) => (item.key === key ? { ...item, error: message } : item))
              : current,
          );
          setNotice(message);
        } finally {
          try { handle?.close(); } catch { /* closed */ }
        }
      }
    },
    [rpc, patch],
  );

  const addPhotos = useCallback(async () => {
    setMenuOpen(false);
    setNotice(null);
    const result = await ImagePicker.launchImageLibraryAsync({
      mediaTypes: ["images"],
      allowsMultipleSelection: true,
      selectionLimit: 6,
      quality: 0.85,
    });
    if (result.canceled) return;
    await send(
      result.assets.map((asset, index) => ({
        uri: asset.uri,
        name: asset.fileName || `photo-${Date.now()}-${index + 1}.${asset.mimeType?.includes("png") ? "png" : "jpg"}`,
        size: asset.fileSize ?? null,
        image: true,
      })),
    );
  }, [send]);

  const addFiles = useCallback(async () => {
    setMenuOpen(false);
    setNotice(null);
    const result = await DocumentPicker.getDocumentAsync({ multiple: true, copyToCacheDirectory: true });
    if (result.canceled) return;
    await send(
      result.assets.map((asset) => ({
        uri: asset.uri,
        name: asset.name,
        size: asset.size ?? null,
        image: (asset.mimeType ?? "").startsWith("image/"),
      })),
    );
  }, [send]);

  const remove = useCallback((key: string) => {
    cancelled.current.add(key);
    setAttachments((current) => current.filter((item) => item.key !== key));
  }, []);

  const uploading = attachments.some((item) => item.path === null && item.error === null);
  const message = useCallback(
    (text: string) => withAttachments(text, attachments.filter((item) => item.path).map((item) => item.path!)),
    [attachments],
  );
  const clear = useCallback(() => setAttachments([]), []);
  const hasAttachments = attachments.some((item) => item.path !== null);

  // ── Models ──
  const [models, setModels] = useState<ChatModels | null>(null);
  const [modelMenu, setModelMenu] = useState(false);
  const [switching, setSwitching] = useState(false);
  useEffect(() => {
    if (!rpc || tabId === null) return;
    let active = true;
    getChatModels(rpc, tabId).then(
      (answer) => { if (active) setModels(answer); },
      () => { if (active) setModels(null); },
    );
    return () => { active = false; };
  }, [rpc, tabId]);
  const pickModel = useCallback(
    async (key: string) => {
      const choice = models?.choices.find((entry) => modelKey(entry.id, entry.effort) === key);
      if (!rpc || !choice || tabId === null) return;
      setSwitching(true);
      try {
        setModels(await setChatModel(rpc, tabId, choice.id, choice.effort));
      } catch {
        setNotice("The model didn't change. Try again.");
      } finally {
        setSwitching(false);
      }
    },
    [rpc, tabId, models],
  );

  return {
    attachments, addPhotos, addFiles, remove, uploading, message, clear, hasAttachments,
    menuOpen, setMenuOpen, notice, setNotice,
    models, modelMenu, setModelMenu, pickModel, switching,
    workspaceRoot,
  };
}

export type ChatExtras = ReturnType<typeof useChatExtras>;

/** The row under the message box: + (attach) and the model. */
export function ChatExtrasBar({ extras, children }: { extras: ChatExtras; children?: ReactNode }) {
  const { theme } = useTheme();
  const { models } = extras;
  const current = models?.current ? models.choices.find((c) => c.id === models.current!.id && c.effort === models.current!.effort) : null;
  return (
    <View style={styles.bar}>
      <Control
        accessibilityRole="button"
        accessibilityLabel="Attach"
        accessibilityState={{ expanded: extras.menuOpen }}
        onPress={() => extras.setMenuOpen(!extras.menuOpen)}
        hitSlop={6}
        style={({ pressed }) => [styles.round, { backgroundColor: pressed || extras.menuOpen ? theme.colors.selection : theme.colors.surface }]}
      >
        <Icon name={extras.menuOpen ? "close" : "plus"} size={18} tone="textMuted" />
      </Control>
      {children}
      {models?.available && models.choices.length > 0 ? (
        <SelectPopover
          label="Model"
          caption="MODEL"
          options={models.choices.map((choice) => ({ id: modelKey(choice.id, choice.effort), label: choice.label }))}
          value={models.current ? modelKey(models.current.id, models.current.effort) : ""}
          onChange={(key) => void extras.pickModel(key)}
          open={extras.modelMenu}
          onOpenChange={extras.setModelMenu}
          searchable={models.choices.length > 8}
          searchPlaceholder="Find a model"
          triggerLabel={extras.switching ? "Switching…" : current?.label ?? models.current?.id ?? "Model"}
          pill
        />
      ) : models?.locked ? (
        <AppText variant="caption" tone="textMuted">Auto (free plan)</AppText>
      ) : null}
    </View>
  );
}

/** Above the message box: the attach menu, attached files, and `@` suggestions. */
export function ChatExtrasTray({ extras, draft, onDraft }: { extras: ChatExtras; draft: string; onDraft: (text: string) => void }) {
  const { theme } = useTheme();
  const { rpc } = useMachine();
  const mention = extras.workspaceRoot ? mentionAtEnd(draft) : null;
  const query = mention?.query ?? null;
  const [found, setFound] = useState<readonly FoundFile[]>([]);
  const [searching, setSearching] = useState(false);
  useEffect(() => {
    if (query === null || query.length === 0 || !rpc || !extras.workspaceRoot) {
      setFound([]);
      return;
    }
    let active = true;
    setSearching(true);
    const timer = setTimeout(() => {
      findWorkspaceFiles(rpc, extras.workspaceRoot!, query).then(
        (files) => { if (active) { setFound(files.slice(0, 20)); setSearching(false); } },
        () => { if (active) { setFound([]); setSearching(false); } },
      );
    }, 220);
    return () => { active = false; clearTimeout(timer); };
  }, [query, rpc, extras.workspaceRoot]);

  // About four rows, less on a short screen.
  const { height: windowHeight } = useWindowDimensions();
  const listHeight = Math.min(240, Math.round(windowHeight * 0.28));
  const relative = (path: string) => {
    const root = extras.workspaceRoot?.replace(/\/+$/, "");
    return root && path.startsWith(`${root}/`) ? path.slice(root.length + 1) : path;
  };

  return (
    <View style={styles.tray} pointerEvents="box-none">
      {query !== null ? (
        <View style={[styles.card, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
          {query.length === 0 ? (
            <AppText variant="caption" tone="textMuted">Type to find a file in this workspace.</AppText>
          ) : searching && found.length === 0 ? (
            <ActivityIndicator size="small" color={theme.colors.textMuted} />
          ) : found.length === 0 ? (
            <AppText variant="caption" tone="textMuted">{`No file matches “${query}”.`}</AppText>
          ) : (
            // A short scrolling list: the card must fit above the message
            // box with the keyboard open, however many files match.
            <ScrollView
              style={{ maxHeight: listHeight }}
              keyboardShouldPersistTaps="handled"
              keyboardDismissMode="none"
              nestedScrollEnabled
              showsVerticalScrollIndicator
            >
            {found.map((file) => (
              <Control
                key={file.path}
                accessibilityRole="button"
                accessibilityLabel={`Reference ${relative(file.path)}`}
                onPress={() => onDraft(completeMention(draft, file.path, extras.workspaceRoot))}
                style={({ pressed }) => [styles.suggestion, pressed ? { backgroundColor: theme.colors.selection } : null]}
              >
                <Icon name={file.folder ? "folder" : "file"} size={15} tone="textMuted" />
                <View style={styles.grow}>
                  <AppText numberOfLines={1}>{file.name}</AppText>
                  <AppText variant="caption" tone="textMuted" numberOfLines={1}>{relative(file.path)}</AppText>
                </View>
              </Control>
            ))}
            </ScrollView>
          )}
        </View>
      ) : null}
      {extras.menuOpen ? (
        <View style={[styles.card, styles.menu, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
          <MenuRow icon="image" label="Photos & screenshots" hint="From your photo library" onPress={() => void extras.addPhotos()} />
          <MenuRow icon="files" label="Files" hint="From this phone or iCloud" onPress={() => void extras.addFiles()} />
          {extras.workspaceRoot ? (
            <MenuRow
              icon="file"
              label="File in this workspace"
              hint="Reference it with @"
              onPress={() => {
                extras.setMenuOpen(false);
                onDraft(draft === "" || /\s$/.test(draft) ? `${draft}@` : `${draft} @`);
              }}
            />
          ) : null}
        </View>
      ) : null}
    </View>
  );
}

/** Attached files, inside the message box above what you type. */
export function ChatAttachmentStrip({ extras }: { extras: ChatExtras }) {
  const { theme } = useTheme();
  if (extras.attachments.length === 0) return null;
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.chips} keyboardShouldPersistTaps="handled">
      {extras.attachments.map((item) => (
        <View key={item.key} style={[styles.chip, { backgroundColor: theme.colors.surface, borderColor: item.error ? theme.colors.danger : theme.colors.border }]}>
          {item.image ? (
            <Image source={{ uri: item.uri }} style={styles.thumb} accessibilityIgnoresInvertColors />
          ) : (
            <View style={[styles.thumb, styles.fileThumb, { backgroundColor: theme.colors.surfaceRaised }]}>
              <Icon name="file" size={16} tone="textMuted" />
            </View>
          )}
          <View style={styles.chipText}>
            <AppText variant="caption" numberOfLines={1}>{item.name}</AppText>
            {item.error ? (
              <AppText variant="caption" tone="danger" numberOfLines={1}>Didn't send</AppText>
            ) : item.path ? null : (
              <View style={[styles.progress, { backgroundColor: theme.colors.surfaceRaised }]}>
                <View style={[styles.progressFill, { width: `${Math.max(4, Math.round(item.progress * 100))}%`, backgroundColor: theme.colors.accent }]} />
              </View>
            )}
          </View>
          <Control
            accessibilityRole="button"
            accessibilityLabel={`Remove ${item.name}`}
            onPress={() => extras.remove(item.key)}
            hitSlop={8}
            style={[styles.remove, { backgroundColor: theme.colors.surfaceRaised }]}
          >
            <Icon name="close" size={12} tone="textMuted" />
          </Control>
        </View>
      ))}
    </ScrollView>
  );
}

function MenuRow({ icon, label, hint, onPress }: { icon: "image" | "files" | "file"; label: string; hint: string; onPress: () => void }) {
  const { theme } = useTheme();
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={label}
      onPress={onPress}
      style={({ pressed }) => [styles.suggestion, pressed ? { backgroundColor: theme.colors.selection } : null]}
    >
      <Icon name={icon} size={17} tone="text" />
      <View style={styles.grow}>
        <AppText>{label}</AppText>
        <AppText variant="caption" tone="textMuted">{hint}</AppText>
      </View>
    </Control>
  );
}

const styles = StyleSheet.create({
  bar: { flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 1, minWidth: 0 },
  round: { width: 38, height: 38, borderRadius: 19, alignItems: "center", justifyContent: "center" },
  tray: { gap: 8 },
  card: { borderWidth: 1, borderRadius: 14, padding: 6, gap: 2 },
  menu: { alignSelf: "flex-start", minWidth: 240 },
  suggestion: { flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 8, paddingVertical: 6, borderRadius: 10, minHeight: 48 },
  grow: { flex: 1, minWidth: 0 },
  chips: { gap: 8, paddingHorizontal: 4, paddingTop: 4, paddingBottom: 2 },
  chip: { flexDirection: "row", alignItems: "center", gap: 8, borderWidth: StyleSheet.hairlineWidth, borderRadius: 14, padding: 6, paddingRight: 8, width: 200 },
  chipText: { flex: 1, minWidth: 0, gap: 4 },
  thumb: { width: 40, height: 40, borderRadius: 9 },
  progress: { height: 3, borderRadius: 2, overflow: "hidden" },
  progressFill: { height: 3, borderRadius: 2 },
  remove: { width: 20, height: 20, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  fileThumb: { alignItems: "center", justifyContent: "center" },
});
