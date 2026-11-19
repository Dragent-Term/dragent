import { useState } from "react";
import { StyleSheet, View } from "react-native";
import { Button, SegmentedControl } from "../../components";
import { useAsyncAction } from "../../hooks/useAsyncAction";
import { spacing } from "../../theme/tokens";
import { usePreview } from "../../preview/PreviewContext";
import type { PreviewAgent } from "../../preview/types";
import { Feedback } from "../shared/Feedback";
import { AGENT_LABEL } from "./PreviewRows";

export interface PreviewTabFormProps {
  workspaceId: string;
  onCreated: (tabId: string) => void;
  onCancel: () => void;
}

type TabKind = "agent" | "terminal" | "browser";

const KIND_OPTIONS = [
  { id: "agent", label: "Agent", hint: "Agent tab" },
  { id: "terminal", label: "Terminal", hint: "Terminal tab" },
  { id: "browser", label: "Browser", hint: "Browser tab" },
] as const;

const AGENT_OPTIONS: readonly { id: PreviewAgent; label: string; hint: string }[] = [
  { id: "dragent", label: "Dragent", hint: "Dragent (built-in) agent tab" },
  { id: "codex", label: "Codex", hint: "Codex agent tab" },
  { id: "opencode", label: "OpenCode", hint: "OpenCode agent tab" },
];

/**
 * Shared add-tab form for the workspace catalog and the leaf add-tab modal.
 * Local only: picks one of the three tab kinds (and, for agents, which agent
 * runs it), then calls core createTab. Failures keep the draft; success hands
 * the new tab id to the caller so it can open the canonical leaf.
 */
export function PreviewTabForm({ workspaceId, onCreated, onCancel }: PreviewTabFormProps) {
  const preview = usePreview();
  const [kind, setKind] = useState<TabKind>("agent");
  const [agent, setAgent] = useState<PreviewAgent>("dragent");

  const create = useAsyncAction(async () => {
    const result =
      kind === "agent"
        ? preview.createTab(workspaceId, { kind: "agent", agent })
        : kind === "terminal"
          ? preview.createTab(workspaceId, { kind: "terminal" })
          : preview.createTab(workspaceId, { kind: "browser" });
    if (!result.ok) throw new Error(result.message);
    onCreated(result.id);
  });

  return (
    <View style={styles.section}>
      <SegmentedControl
        label="Tab kind"
        options={[...KIND_OPTIONS]}
        value={kind}
        onChange={setKind}
      />
      {kind === "agent" ? (
        <SegmentedControl
          label="Agent"
          options={[...AGENT_OPTIONS]}
          value={agent}
          onChange={setAgent}
        />
      ) : null}
      <View style={styles.actions}>
        <Button
          label={kind === "agent" ? `Create ${AGENT_LABEL[agent]} tab` : kind === "terminal" ? "Create terminal tab" : "Create browser tab"}
          loading={create.pending}
          onPress={() => { void create.run(); }}
        />
        <Button label="Cancel" variant="ghost" onPress={onCancel} />
      </View>
      <Feedback message={create.error} />
    </View>
  );
}

const styles = StyleSheet.create({
  section: { gap: spacing.sm },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
});
