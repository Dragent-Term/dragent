import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, FlatList, ScrollView, StyleSheet, View } from "react-native";
import { AppText, Control, EmptyState, Icon, SearchField } from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import type { RelayRpcClient } from "../../transport/relayRpcClient";
import { listFolder, searchFolder, type RelayRpcFileEntry } from "../../transport/relayRpcMethods";
import { crumbs, joinPath, relativeTo, sortEntries } from "./paths";

interface Row {
  readonly name: string;
  readonly kind: "file" | "directory";
  readonly path: string;
  /** Where a search hit lives, relative to the workspace. */
  readonly where: string | null;
}

function messageOf(failure: unknown, fallback: string): string {
  const text = (failure as { message?: unknown } | null)?.message;
  return typeof text === "string" && text !== "" ? text : fallback;
}

/**
 * The workspace's files as they are on the desktop: folders first, tap to
 * go in, breadcrumbs to go back, and a name search over the whole
 * workspace. Choosing a file hands its path to `onOpen`.
 */
export function FilesPanel({ rpc, root, onOpen }: {
  rpc: RelayRpcClient;
  root: string;
  onOpen: (path: string) => void;
}) {
  const { theme } = useTheme();
  const [folder, setFolder] = useState(root);
  const [rows, setRows] = useState<readonly Row[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [attempt, setAttempt] = useState(0);
  const request = useRef(0);
  const searching = query.trim().length >= 2;

  useEffect(() => {
    const id = ++request.current;
    setRows(null);
    setError(null);
    const text = query.trim();
    const run = () => {
      const work: Promise<readonly RelayRpcFileEntry[]> = searching ? searchFolder(rpc, root, text) : listFolder(rpc, folder);
      work
        .then((entries) => {
          if (request.current !== id) return;
          const mapped: Row[] = entries.map((entry) => {
            const path = entry.path ?? joinPath(folder, entry.name);
            const relative = searching ? relativeTo(root, path) : null;
            const cut = relative ? relative.lastIndexOf("/") : -1;
            return { name: entry.name, kind: entry.kind, path, where: relative && cut > 0 ? relative.slice(0, cut) : null };
          });
          setRows(searching ? mapped.filter((row) => relativeTo(root, row.path) !== null) : sortEntries(mapped));
        })
        .catch((failure: unknown) => {
          if (request.current !== id) return;
          setError(messageOf(failure, "The desktop couldn't read this folder."));
        });
    };
    // Typing a search waits a beat; opening a folder doesn't.
    const timer = setTimeout(run, searching ? 250 : 0);
    return () => clearTimeout(timer);
  }, [rpc, root, folder, query, searching, attempt]);

  const open = useCallback((row: Row) => {
    if (row.kind === "directory") {
      setQuery("");
      setFolder(row.path);
    } else onOpen(row.path);
  }, [onOpen]);

  const trail = crumbs(root, folder);

  return (
    <View style={styles.flex}>
      <View style={styles.search}>
        <SearchField label="Search files in this workspace" placeholder="Search files by name" value={query} onChangeText={setQuery} />
      </View>
      {searching ? null : (
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator={false}
          style={styles.crumbs}
          contentContainerStyle={styles.crumbsContent}
          accessibilityLabel="Folder path"
        >
          {trail.map((crumb, index) => {
            const last = index === trail.length - 1;
            return (
              <View key={crumb.path} style={styles.crumb}>
                {index > 0 ? <Icon name="chevron-right" size={12} tone="textSubtle" /> : null}
                <Control
                  accessibilityRole="button"
                  accessibilityLabel={`Go to ${crumb.label}`}
                  disabled={last}
                  onPress={() => setFolder(crumb.path)}
                  style={styles.crumbButton}
                >
                  <AppText variant="caption" tone={last ? "text" : "accent"} style={last ? styles.strong : null}>{crumb.label}</AppText>
                </Control>
              </View>
            );
          })}
        </ScrollView>
      )}
      {error ? (
        <EmptyState title="Can't show these files" message={error} actionLabel="Try again" onAction={() => setAttempt((n) => n + 1)} />
      ) : rows === null ? (
        <View style={styles.center}><ActivityIndicator color={theme.colors.accent} /></View>
      ) : (
        <FlatList
          data={rows}
          keyExtractor={(row) => row.path}
          keyboardShouldPersistTaps="handled"
          keyboardDismissMode="on-drag"
          contentContainerStyle={rows.length === 0 ? styles.grow : styles.listContent}
          renderItem={({ item }) => (
            <Control
              accessibilityRole="button"
              accessibilityLabel={`${item.kind === "directory" ? "Folder" : "File"} ${item.name}`}
              onPress={() => open(item)}
              style={({ pressed, hovered }) => [
                styles.row,
                { borderColor: theme.colors.border },
                pressed || hovered ? { backgroundColor: theme.colors.surfaceRaised } : null,
              ]}
            >
              <Icon name={item.kind === "directory" ? "folder" : "files"} size={16} tone={item.kind === "directory" ? "accent" : "textMuted"} />
              <View style={styles.flex}>
                <AppText numberOfLines={1}>{item.name}</AppText>
                {item.where ? <AppText variant="caption" tone="textSubtle" numberOfLines={1}>{item.where}</AppText> : null}
              </View>
              {item.kind === "directory" ? <Icon name="chevron-right" size={14} tone="textSubtle" /> : null}
            </Control>
          )}
          ListEmptyComponent={
            <EmptyState
              title={searching ? "No files match" : "This folder is empty"}
              message={searching ? "Try a different name. Dependency and build folders aren't searched." : "Nothing is in this folder on your desktop."}
            />
          }
        />
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1, minWidth: 0, minHeight: 0 },
  grow: { flexGrow: 1 },
  strong: { fontWeight: "600" },
  center: { flex: 1, alignItems: "center", justifyContent: "center" },
  search: { paddingHorizontal: spacing.md, paddingTop: spacing.sm },
  crumbs: { flexGrow: 0 },
  crumbsContent: { alignItems: "center", paddingHorizontal: spacing.md },
  crumb: { flexDirection: "row", alignItems: "center", gap: 2 },
  crumbButton: { minHeight: 40, justifyContent: "center", paddingHorizontal: 4 },
  listContent: { paddingBottom: spacing.xl },
  row: {
    minHeight: 46,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    paddingHorizontal: spacing.lg,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
});
