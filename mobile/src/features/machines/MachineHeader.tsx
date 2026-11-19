import { StyleSheet, View } from "react-native";
import { AgentStateDot } from "../../components/AgentStateDot";
import { CompanionHeader, type CompanionHeaderProps } from "../../components/CompanionHeader";

export interface MachineHeaderProps extends Omit<CompanionHeaderProps, "status"> {
  online: boolean | null;
}

/**
 * A desktop's screens share this header. Its connection shows as a dot (green
 * online, grey offline), right beside the header's actions; until the
 * catalog loads there's no dot.
 */
export function MachineHeader({ online, actions, ...props }: MachineHeaderProps) {
  const dot =
    online === null ? null : (
      <View
        accessible
        accessibilityRole="text"
        accessibilityLabel={online ? "Desktop online" : "Desktop offline"}
        style={styles.dot}
      >
        <AgentStateDot state={online ? "online" : "idle"} />
      </View>
    );
  return (
    <CompanionHeader
      {...props}
      actions={
        dot || actions ? (
          <View style={styles.trailing}>
            {dot}
            {actions}
          </View>
        ) : undefined
      }
    />
  );
}

const styles = StyleSheet.create({
  trailing: { flexDirection: "row", alignItems: "center", gap: 2 },
  dot: { minHeight: 36, justifyContent: "center", paddingLeft: 4 },
});
