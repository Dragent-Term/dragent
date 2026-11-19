/**
 * The agent card on Android: an ongoing notification.
 *
 * The layout runs in the widget's own runtime ('widget' directive), where
 * the UI components and modifiers exist only under their own names: import
 * them unrenamed, and use nothing else from the app inside the layout.
 */
import { Column, Row, Text } from "@expo/ui/jetpack-compose";
import { padding, weight } from "@expo/ui/jetpack-compose/modifiers";
import { createCustomNotification } from "expo-widgets";
import type { AgentCardProps } from "./props";

export const AgentCardNotification = createCustomNotification<AgentCardProps>("AgentCard", (props) => {
  "widget";
  const colorFor = (state: string) =>
    state === "needs_input" ? "#F59E0B" : state === "failed" ? "#EF4444" : state === "review" ? "#22C55E" : "#3B82F6";
  const minutes = (since: number) => {
    const m = Math.max(0, Math.floor((Date.now() - since) / 60000));
    return m < 1 ? "now" : m < 60 ? `${m} min` : `${Math.floor(m / 60)} h`;
  };
  const row = (item: AgentCardProps["rows"][number]) => (
    <Row key={item.key} verticalAlignment="center" modifiers={[padding(0, 4, 0, 4)]}>
      <Text color={colorFor(item.state)} style={{ fontSize: 14, fontWeight: "bold" }}>●</Text>
      <Column modifiers={[weight(1), padding(8, 0, 8, 0)]}>
        <Text maxLines={1} style={{ fontSize: 14, fontWeight: "bold" }}>{item.title}</Text>
        <Text maxLines={1} color="#9CA3AF" style={{ fontSize: 12 }}>{item.place}</Text>
      </Column>
      <Column horizontalAlignment="end">
        <Text color={colorFor(item.state)} style={{ fontSize: 12, fontWeight: "bold" }}>{item.label}</Text>
        <Text color="#9CA3AF" style={{ fontSize: 11 }}>{minutes(item.since)}</Text>
      </Column>
    </Row>
  );
  const top = props.rows[0];
  return {
    collapsed: (
      <Column>
        <Text maxLines={1} style={{ fontSize: 14, fontWeight: "bold" }}>{top ? `${top.title} · ${top.label}` : "Dragent"}</Text>
        <Text maxLines={1} color="#9CA3AF" style={{ fontSize: 12 }}>{props.summary}</Text>
      </Column>
    ),
    expanded: (
      <Column>
        <Text color="#9CA3AF" style={{ fontSize: 12 }}>{props.summary}</Text>
        {props.rows.slice(0, 4).map(row)}
      </Column>
    ),
  };
});
