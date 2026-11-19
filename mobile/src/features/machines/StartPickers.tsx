import { useEffect, useState } from "react";
import { StyleSheet, View } from "react-native";
import { AgentMark, AppText, Control, Icon } from "../../components";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";
import type { RelayRpcClient } from "../../transport/relayRpcClient";
import type { StartAgent, TaskOptions } from "../../tasks/types";

/**
 * The desktop's projects, agents, members and labels for a start form or a
 * picker, loaded once per screen. `null` while loading; failures keep
 * `null` with a message.
 */
export function useStartOptions(
  rpc: RelayRpcClient | null,
  load: (client: RelayRpcClient) => Promise<TaskOptions>,
): { options: TaskOptions | null; error: string | null; reload: () => void } {
  const [options, setOptions] = useState<TaskOptions | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (rpc === null) return;
    let cancelled = false;
    setError(null);
    load(rpc).then(
      (loaded) => !cancelled && setOptions(loaded),
      () => !cancelled && setError("Couldn't load the desktop's projects and agents. Check it's online, then try again."),
    );
    return () => {
      cancelled = true;
    };
    // `load` is a module function; reload with `attempt`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rpc, attempt]);
  return { options, error, reload: () => setAttempt((n) => n + 1) };
}

/**
 * Agents as chips, each with its mark beside its name. `single` picks one
 * (a lead, the agent for a task); otherwise any number toggle (workers).
 */
export function AgentChips({
  label,
  agents,
  selected,
  onChange,
  single = false,
  disabled = false,
}: {
  label: string;
  agents: readonly StartAgent[];
  selected: readonly string[];
  onChange: (next: string[]) => void;
  single?: boolean;
  disabled?: boolean;
}) {
  const { theme } = useTheme();
  return (
    <View style={styles.group} accessibilityRole={single ? "radiogroup" : undefined} accessibilityLabel={label}>
      <AppText variant="caption" tone="textMuted">{label}</AppText>
      <View style={styles.chips}>
        {agents.map((agent) => {
          const on = selected.includes(agent.key);
          return (
            <Control
              key={agent.key}
              accessibilityRole={single ? "radio" : "checkbox"}
              accessibilityState={single ? { selected: on, disabled } : { checked: on, disabled }}
              accessibilityLabel={agent.label}
              disabled={disabled}
              onPress={() =>
                onChange(single ? [agent.key] : on ? selected.filter((key) => key !== agent.key) : [...selected, agent.key])
              }
              style={({ pressed, hovered }) => [
                styles.chip,
                {
                  borderColor: on ? theme.colors.accent : theme.colors.border,
                  backgroundColor: on ? theme.colors.surfaceRaised : pressed || hovered ? theme.colors.surface : undefined,
                },
              ]}
            >
              <AgentMark agent={agent.key} size={16} />
              <AppText variant="caption" numberOfLines={1}>{agent.label}</AppText>
              {on && !single ? <Icon name="check" size={14} tone="accent" /> : null}
            </Control>
          );
        })}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  group: { gap: spacing.xs },
  chips: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs },
  chip: {
    flexDirection: "row",
    alignItems: "center",
    gap: 6,
    minHeight: 40,
    paddingHorizontal: spacing.sm,
    borderRadius: 999,
    borderWidth: 1,
  },
});
