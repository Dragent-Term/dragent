import { useCallback, useState, type ReactNode } from "react";
import { Alert, StyleSheet, View } from "react-native";
import { AgentStateDot } from "../../components";
import { ActionSheet, type SheetAction } from "../../components/ActionSheet";
import { confirmAction } from "../../components/confirmAction";
import { needsSummary, statusDot } from "../../machines/attention";
import { useMachine } from "../../machines/MachineContext";
import { setPinned } from "../../machines/pins";
import type { DirectoryWorkspace } from "../../machines/workspaceDirectory";
import { deleteWorkspace, type WorkspaceDeleteResult } from "../../transport/relayRpcMethods";
import { blockedMessage } from "../../machines/workspaceDelete";

/** How long the action sheet takes to fade out. */
const SHEET_CLOSE_MS = 350;

/** The desktop's own id for a workspace: only those can be deleted from here. */
function deletableId(workspace: DirectoryWorkspace): string | null {
  return /^[A-Za-z0-9]{1,64}$/.test(workspace.key) ? workspace.key : null;
}

/**
 * A workspace's long-press actions: pin or unpin it (on this phone), and
 * delete it on the desktop. Deleting asks first; if agents still work there
 * or a worktree has uncommitted changes, the desktop says so and the person
 * chooses whether to delete anyway. The row hides at once and comes back if
 * the desktop doesn't delete it.
 */
export function useWorkspaceActions(deviceId: string, pinned: readonly string[]): {
  open: (workspace: DirectoryWorkspace) => void;
  /** Rows to leave out while their delete is under way. */
  hidden: ReadonlySet<string>;
  sheet: ReactNode;
} {
  const { rpc } = useMachine();
  const [target, setTarget] = useState<DirectoryWorkspace | null>(null);
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set());

  const hide = useCallback((key: string, on: boolean) => {
    setHidden((prev) => {
      const next = new Set(prev);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });
  }, []);

  const runDelete = useCallback(
    async (workspace: DirectoryWorkspace, id: string, force: boolean) => {
      if (!rpc) {
        Alert.alert("Couldn't delete the workspace", "This desktop isn't connected right now.");
        return;
      }
      hide(workspace.key, true);
      let result: WorkspaceDeleteResult;
      try {
        result = await deleteWorkspace(rpc, id, force);
      } catch (error) {
        hide(workspace.key, false);
        const reason = error instanceof Error && error.message ? error.message : "The desktop didn't answer.";
        Alert.alert("Couldn't delete the workspace", reason);
        return;
      }
      if (result.deleted) {
        // The catalog drops it; the pin goes too.
        void setPinned(deviceId, workspace.key, false);
        return;
      }
      hide(workspace.key, false);
      confirmAction({
        title: `Delete ${workspace.name} anyway?`,
        message: blockedMessage(result),
        actionLabel: "Delete anyway",
        actionStyle: "destructive",
        onConfirm: () => void runDelete(workspace, id, true),
      });
    },
    [rpc, deviceId, hide],
  );

  const askDelete = useCallback(
    (workspace: DirectoryWorkspace) => {
      const id = deletableId(workspace);
      if (!id) return;
      confirmAction({
        title: `Delete ${workspace.name}?`,
        message: workspace.mainCheckout
          ? "It's the project's own checkout, so it only closes in Dragent. Its files stay on the desktop."
          : `Its tabs close and its worktree folder (${workspace.branch}) is removed from the desktop.`,
        actionLabel: "Delete",
        actionStyle: "destructive",
        onConfirm: () => void runDelete(workspace, id, false),
      });
    },
    [runDelete],
  );

  const isPinned = target ? pinned.includes(target.key) : false;
  const actions: SheetAction[] = target
    ? [
        {
          label: isPinned ? "Unpin" : "Pin to top",
          detail: isPinned ? undefined : "Keep it above the other workspaces on this phone",
          icon: isPinned ? "pin-off" : "pin",
          onPress: () => void setPinned(deviceId, target.key, !isPinned),
        },
        ...(deletableId(target)
          ? [{
              label: "Delete workspace",
              detail: target.mainCheckout ? "Closes it in Dragent; files stay" : "Removes its worktree on the desktop",
              icon: "trash" as const,
              destructive: true,
              // After the sheet has gone: iOS won't show an alert over a closing modal.
              onPress: () => setTimeout(() => askDelete(target), SHEET_CLOSE_MS),
            }]
          : []),
      ]
    : [];
  const dot = target ? statusDot(target.needs) : null;
  const sheet = (
    <ActionSheet
      visible={target !== null}
      title={target?.name ?? ""}
      subtitle={target ? [target.projectName, target.branch, needsSummary(target.needs)].filter(Boolean).join(" · ") : undefined}
      header={dot ? <View style={styles.dot}><AgentStateDot state={dot} /></View> : undefined}
      actions={actions}
      onClose={() => setTarget(null)}
    />
  );
  return { open: setTarget, hidden, sheet };
}

const styles = StyleSheet.create({
  dot: { width: 14, alignItems: "center" },
});
