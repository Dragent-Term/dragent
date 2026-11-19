import { useEffect, useState } from "react";
import { StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { Button, FormField, OptionRow, SelectPopover } from "../../components";
import { ScrollScreen } from "../../layout";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import { machinePath, machineTaskPath } from "../../machines/routes";
import { MachineHeader } from "./MachineHeader";
import { Feedback } from "../shared/Feedback";
import { createTask, listMembers } from "../../tasks/rpc";
import {
  TASK_PRIORITIES,
  TASK_PRIORITY_LABEL,
  TASK_STATUSES,
  TASK_STATUS_META,
  type TaskPerson,
  type TaskPriority,
  type TaskStatus,
} from "../../tasks/types";

const NOBODY = "__nobody__";

/** A new task for the organisation, created through the paired desktop. */
export function MachineNewTaskScreen({ deviceId }: { deviceId: string }) {
  const { theme } = useTheme();
  const machine = useMachine();
  const rpc = machine.rpc;
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [status, setStatus] = useState<TaskStatus>("todo");
  const [priority, setPriority] = useState<TaskPriority>("none");
  const [assignee, setAssignee] = useState(NOBODY);
  const [members, setMembers] = useState<readonly TaskPerson[]>([]);
  const [menu, setMenu] = useState<"status" | "priority" | "assignee" | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (rpc === null) return;
    let cancelled = false;
    listMembers(rpc).then((list) => !cancelled && setMembers(list), () => undefined);
    return () => {
      cancelled = true;
    };
  }, [rpc]);

  const back = machinePath(deviceId);
  const toList = () => router.navigate({ pathname: back.pathname, params: { ...back.params, view: "tasks" } });

  const create = async () => {
    if (rpc === null || !title.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const task = await createTask(rpc, {
        title,
        description,
        status,
        priority,
        ...(assignee === NOBODY ? {} : { assigneeId: assignee }),
      });
      router.replace(machineTaskPath(deviceId, task.key));
    } catch {
      setError("The task wasn't created. Check the desktop is online and signed in, then try again.");
      setBusy(false);
    }
  };

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <MachineHeader
        title="New task"
        subtitle="For your organisation"
        online={machine.catalog.status === "live" ? machine.catalog.machine.online : null}
        onBack={toList}
        backLabel="Back to Tasks"
      />
      <ScrollScreen maxWidth={720} contentStyle={styles.content}>
        <FormField label="Title" placeholder="What needs doing?" value={title} onChangeText={setTitle} maxLength={500} autoFocus />
        <FormField
          label="Description"
          placeholder="Details, links, acceptance criteria… (optional)"
          value={description}
          onChangeText={setDescription}
          multiline
          maxLength={20_000}
        />
        <OptionRow accessibilityLabel="Task details">
          <SelectPopover
            label="Status"
            caption="STATUS"
            options={TASK_STATUSES.filter((key) => key !== "done" && key !== "canceled").map((key) => ({ id: key, label: TASK_STATUS_META[key].label }))}
            value={status}
            onChange={setStatus}
            open={menu === "status"}
            onOpenChange={(open) => setMenu(open ? "status" : null)}
          />
          <SelectPopover
            label="Priority"
            caption="PRIORITY"
            options={TASK_PRIORITIES.map((key) => ({ id: key, label: TASK_PRIORITY_LABEL[key] }))}
            value={priority}
            onChange={setPriority}
            open={menu === "priority"}
            onOpenChange={(open) => setMenu(open ? "priority" : null)}
          />
          <SelectPopover
            label="Assignee"
            caption="ASSIGN TO"
            options={[{ id: NOBODY, label: "Unassigned" }, ...members.map((member) => ({ id: member.id, label: member.name }))]}
            value={assignee}
            onChange={setAssignee}
            open={menu === "assignee"}
            onOpenChange={(open) => setMenu(open ? "assignee" : null)}
            searchable={members.length > 8}
            searchPlaceholder="Find a member"
          />
        </OptionRow>
        <Feedback message={error} />
        <View style={styles.actions}>
          <Button label="Cancel" size="sm" variant="ghost" onPress={toList} />
          <Button label="Create task" size="sm" loading={busy} disabled={!title.trim() || busy || rpc === null} onPress={() => void create()} />
        </View>
      </ScrollScreen>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  content: { gap: spacing.md },
  actions: { flexDirection: "row", justifyContent: "flex-end", gap: spacing.sm },
});
