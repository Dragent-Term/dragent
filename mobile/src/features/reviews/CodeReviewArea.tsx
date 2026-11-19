import { useCallback, useMemo, useState } from "react";
import { FlatList, StyleSheet, View, useWindowDimensions } from "react-native";
import { router, useFocusEffect } from "expo-router";
import {
  AppText,
  Avatar,
  Button,
  Control,
  EmptyState,
  Icon,
  IconButton,
  SearchField,
  SelectPopover,
  StatusBadge,
  OptionRow,
} from "../../components";
import { Screen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import {
  REVIEW_SCOPES,
  REVIEW_STATES,
  filterPullRequests,
  pullRequestReadiness,
} from "../../preview/pullRequestSelectors";
import type { PreviewPullRequest, ReviewScope, ReviewState } from "../../preview/pullRequestTypes";
import { NewPullRequestSheet } from "./NewPullRequestSheet";
import { PullRequestDetail } from "./PullRequestDetail";
import { projectName, reviewReference, timeAgo } from "./pullRequestData";

type PickerKey = "project" | "state" | "scope" | null;

function ReviewRow({ review, project, selected, showChevron, onOpen }: {
  review: PreviewPullRequest;
  project: string;
  selected: boolean;
  showChevron: boolean;
  onOpen: () => void;
}) {
  const { theme } = useTheme();
  const readiness = pullRequestReadiness(review);
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={`${review.title}, ${project} ${reviewReference(review)}, ${readiness.label}`}
      accessibilityState={{ selected }}
      onPress={onOpen}
      style={({ pressed, hovered }) => [
        styles.row,
        {
          borderColor: theme.colors.border,
          backgroundColor: selected ? theme.colors.surfaceRaised
            : pressed || hovered ? theme.colors.surface : undefined,
          borderLeftColor: selected ? theme.colors.accent : "transparent",
        },
      ]}
    >
      <Icon name="pull-request" size={16} tone={review.state === "open" && !review.isDraft ? "success" : "textMuted"} />
      <View style={styles.copy}>
        <AppText style={styles.name} numberOfLines={2}>{review.title}</AppText>
        <AppText variant="caption" tone="textMuted" numberOfLines={1}>
          {`${project} · ${reviewReference(review)}`}
        </AppText>
        <View style={styles.metaRow}>
          <Avatar name={review.author} size={16} />
          <AppText variant="caption" tone="textMuted" numberOfLines={1} style={styles.metaAuthor}>
            {review.author}
          </AppText>
          <AppText variant="caption" tone="textSubtle" numberOfLines={1}>{timeAgo(review.updatedAt)}</AppText>
        </View>
        <StatusBadge label={readiness.label} tone={readiness.tone} />
      </View>
      {showChevron ? <Icon name="chevron-right" size={16} /> : null}
    </Control>
  );
}

/**
 * Machine-wide PR/MR inbox, mirroring desktop `render()` + `inbox()`.
 * Narrow screens navigate to the request-detail route; wide screens
 * (~1050px+) split inbox and detail with the same detail renderer.
 */
export function CodeReviewArea({ desktopId }: { desktopId: string }) {
  const { theme } = useTheme();
  const preview = usePreview();
  const { width } = useWindowDimensions();
  const wide = width >= 1050;
  const compact = width < 480;

  const [projectId, setProjectId] = useState("");
  const [state, setState] = useState<ReviewState>("open");
  const [scope, setScope] = useState<ReviewScope>("all");
  const [query, setQuery] = useState("");
  const [openPicker, setOpenPicker] = useState<PickerKey>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshNote, setRefreshNote] = useState<string | null>(null);
  const [pageSize, setPageSize] = useState(10);

  // Never leave a stale sheet behind when navigating away and back.
  useFocusEffect(useCallback(() => () => { setSheetOpen(false); setOpenPicker(null); }, []));

  const desktop = preview.desktops.find((entry) => entry.id === desktopId) ?? null;
  const projects = useMemo(
    () => preview.projects.filter((project) => project.desktopId === desktopId),
    [preview, desktopId],
  );
  const total = useMemo(() => filterPullRequests(preview, desktopId, { state: "all" }).length, [preview, desktopId]);
  const allVisible = useMemo(
    () => filterPullRequests(preview, desktopId, {
      projectId: projectId || undefined,
      state,
      scope,
      query,
    }),
    [preview, desktopId, projectId, state, scope, query],
  );
  const visible = useMemo(() => allVisible.slice(0, pageSize), [allVisible, pageSize]);
  const hasMore = allVisible.length > visible.length;
  const selected = allVisible.find((review) => review.id === selectedId) ?? allVisible[0] ?? null;

  const refresh = () => {
    setRefreshing(true);
    setRefreshNote(null);
    // Local preview has no provider fetch; re-run selectors on next frame.
    requestAnimationFrame(() => {
      setPageSize(10);
      setRefreshing(false);
      setRefreshNote("Preview refreshed locally. No provider was contacted.");
    });
  };

  if (!desktop) {
    return (
      <Screen maxWidth={1040}>
        <EmptyState title="Machine not found" message="This sample machine is no longer available." />
      </Screen>
    );
  }

  const handlePopover = (key: Exclude<PickerKey, null>) => (next: boolean) => setOpenPicker(next ? key : null);
  const resetFilters = () => {
    setProjectId("");
    setState("open");
    setScope("all");
    setQuery("");
    setOpenPicker(null);
  };

  const openReview = (id: string) => {
    setSelectedId(id);
    if (!wide) {
      router.push({ pathname: "/preview/desktop/[desktopId]/review/[reviewId]", params: { desktopId, reviewId: id } });
    }
  };

  const projectOptions = [{ id: "", label: "All projects" },
    ...projects.map((project) => ({ id: project.id, label: project.name }))];

  // Three desktop-style popovers in one row; each shows its whole label and
  // the row scrolls sideways when they don't fit.
  const pickers = (
    <OptionRow accessibilityLabel="Review filters">
      <SelectPopover label="Project" caption="PROJECTS"
        options={projectOptions} value={projectId} onChange={setProjectId}
        open={openPicker === "project"} onOpenChange={handlePopover("project")}
        searchable searchPlaceholder="Search projects" />
      <SelectPopover label="State" caption="REVIEW STATE"
        options={REVIEW_STATES} value={state} onChange={setState}
        open={openPicker === "state"} onOpenChange={handlePopover("state")} />
      <SelectPopover label="Scope" caption="SHOW REVIEWS"
        options={REVIEW_SCOPES} value={scope} onChange={setScope}
        open={openPicker === "scope"} onOpenChange={handlePopover("scope")} />
    </OptionRow>
  );

  const controls = (
    <View style={[styles.controls, compact ? styles.controlsCompact : null]}>
      <View style={styles.titleBlock}>
        <View style={[styles.titleRow, compact ? styles.titleRowCompact : null]}>
          <View style={[compact ? styles.titleIconCompact : styles.titleIcon, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
            <Icon name="pull-request" size={compact ? 16 : 20} tone="accent" />
          </View>
          <View style={styles.titleCopy}>
            <View style={styles.titleLine}>
              <AppText variant={compact ? "body" : "title"} accessibilityRole="header" style={compact ? styles.titleCompact : undefined}>Code reviews</AppText>
              <View style={[styles.count, compact ? styles.countCompact : null, { backgroundColor: theme.colors.surfaceRaised }]}>
                <AppText variant="caption" tone="textMuted">{`${total}`}</AppText>
              </View>
            </View>
            <AppText variant="caption" tone="textMuted" numberOfLines={compact ? 1 : undefined}>Pull requests and merge requests across your projects</AppText>
            {refreshNote ? (
              <AppText variant="caption" tone="textSubtle" numberOfLines={2}>{refreshNote}</AppText>
            ) : null}
          </View>
          <View style={styles.titleActions}>
            <IconButton name="refresh" size="sm" label={refreshing ? "Refreshing reviews" : "Refresh reviews"} onPress={refresh} disabled={refreshing} />
            <Button label={compact && width < 380 ? "New" : "New pull request"} size="sm"
              leading={<Icon name="plus" size={14} tone="onAccent" />} onPress={() => setSheetOpen(true)} />
          </View>
        </View>
      </View>
      <SearchField
        label="Search reviews"
        placeholder="Search title, branch, author, #number…"
        value={query}
        onChangeText={setQuery}
      />
      {pickers}
    </View>
  );

  const list = (
    <FlatList
      data={visible}
      keyExtractor={(review) => review.id}
      style={styles.list}
      contentContainerStyle={styles.listContent}
      keyboardShouldPersistTaps="handled"
      ListHeaderComponent={
        <View style={styles.controls}>
          {wide ? null : controls}
          <View style={styles.inboxHeader}>
          <AppText style={styles.inboxTitle}>Inbox</AppText>
          <AppText variant="caption" tone="textMuted">{`${visible.length} review${visible.length === 1 ? "" : "s"}`}</AppText>
          </View>
        </View>
      }
      renderItem={({ item }) => (
        <ReviewRow
          review={item}
          project={projectName(preview, item.projectId)}
          selected={selected?.id === item.id}
          showChevron={!wide}
          onOpen={() => openReview(item.id)}
        />
      )}
      ListFooterComponent={
        hasMore ? (
          <View style={styles.moreWrap}>
            <Button label={refreshing ? "Refreshing…" : `Load more reviews (${allVisible.length - visible.length} more)`}
              variant="ghost" size="sm" disabled={refreshing} onPress={() => setPageSize((n) => n + 10)} />
          </View>
        ) : undefined
      }
      ListEmptyComponent={
        total === 0 ? (
          <EmptyState
            icon={<Icon name="pull-request" size={32} tone="accent" />}
            title="No reviews yet"
            message="Sample pull and merge requests for this machine will appear here. You can also create a local sample."
          />
        ) : (
          <EmptyState
            title="No reviews match these filters"
            message="Try another project, state, or search."
            actionLabel="Show all"
            onAction={resetFilters}
          />
        )
      }
    />
  );

  return (
    <Screen maxWidth={1200} contentStyle={[styles.content, compact ? styles.contentCompact : null]}>
      {wide ? controls : null}
      {wide ? (
        <View style={styles.split}>
          <View style={[styles.inboxPane, { width: width < 1100 ? 280 : 340, borderColor: theme.colors.border }]}>
            {list}
          </View>
          <View style={styles.dividerWrap}>
            <View style={[styles.divider, { backgroundColor: theme.colors.border }]} />
          </View>
          <View style={styles.detailPane}>
            {selected ? (
              <PullRequestDetail key={selected.id} desktopId={desktopId} reviewId={selected.id} />
            ) : (
              <View style={styles.chooseWrap}>
                <EmptyState
                  title="Choose a review"
                  message="Open a pull or merge request to see its description, code, checks, conversations and merge requirements."
                />
              </View>
            )}
          </View>
        </View>
      ) : (
        list
      )}
      <NewPullRequestSheet
        desktopId={desktopId}
        open={sheetOpen}
        onClose={() => setSheetOpen(false)}
          onCreated={(id) => { setSheetOpen(false); resetFilters(); openReview(id); }}
      />
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { gap: spacing.md },
  contentCompact: { gap: spacing.sm },
  controls: { gap: spacing.md },
  controlsCompact: { gap: spacing.sm },
  titleBlock: { gap: spacing.sm },
  titleRow: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.md },
  titleRowCompact: { gap: spacing.sm },
  titleActions: { flexDirection: "row", alignItems: "center", gap: spacing.sm, flexShrink: 0 },
  moreWrap: { paddingVertical: spacing.sm, alignItems: "center" },
  titleIcon: { width: 40, height: 40, borderRadius: 12, borderWidth: 1, alignItems: "center", justifyContent: "center", flexShrink: 0 },
  titleIconCompact: { width: 32, height: 32, borderRadius: 10, borderWidth: 1, alignItems: "center", justifyContent: "center", flexShrink: 0 },
  titleCopy: { flexGrow: 1, flexShrink: 1, flexBasis: 160, minWidth: 0, gap: 2 },
  titleLine: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  titleCompact: { fontWeight: "700" },
  count: { minWidth: 28, height: 24, paddingHorizontal: spacing.sm, borderRadius: 12, alignItems: "center", justifyContent: "center" },
  countCompact: { minWidth: 24, height: 20 },
  split: { flex: 1, minHeight: 0, flexDirection: "row" },
  inboxPane: { borderRightWidth: 1, minHeight: 0 },
  dividerWrap: { width: 1, minHeight: 0 },
  divider: { flex: 1, width: 1 },
  detailPane: { flex: 1, minWidth: 0, minHeight: 0 },
  chooseWrap: { flex: 1, padding: spacing.lg, justifyContent: "center" },
  list: { flex: 1, minHeight: 0 },
  listContent: { paddingBottom: spacing.xl, flexGrow: 1 },
  inboxHeader: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingVertical: spacing.xs },
  inboxTitle: { fontWeight: "600" },
  row: { minHeight: 56, flexDirection: "row", gap: spacing.sm, paddingVertical: spacing.sm, paddingHorizontal: spacing.sm, paddingLeft: spacing.md, borderBottomWidth: 1, borderLeftWidth: 2, alignItems: "flex-start" },
  copy: { flex: 1, minWidth: 0, gap: 2 },
  name: { fontWeight: "600" },
  metaRow: { flexDirection: "row", alignItems: "center", gap: 6 },
  metaAuthor: { flexShrink: 1 },
});
