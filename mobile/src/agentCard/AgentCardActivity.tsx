/**
 * The agent card on iOS: a Live Activity (lock screen and Dynamic Island).
 *
 * The layout runs in the widget's own runtime ('widget' directive), where
 * the UI components and modifiers exist only under their own names: import
 * them unrenamed, and use nothing else from the app inside the layout.
 */
import { HStack, Image, Spacer, Text, VStack } from "@expo/ui/swift-ui";
import { activityBackgroundTint, font, foregroundStyle, lineLimit, padding } from "@expo/ui/swift-ui/modifiers";
import { createLiveActivity } from "expo-widgets";
import type { AgentCardProps } from "./props";

export const AgentCardActivity = createLiveActivity<AgentCardProps>("AgentCard", (props) => {
  "widget";
  const symbolFor = (state: string) =>
    state === "needs_input" ? "exclamationmark.bubble.fill"
    : state === "failed" ? "xmark.octagon.fill"
    : state === "review" ? "checkmark.circle.fill"
    : "circle.dotted.circle";
  const colorFor = (state: string) =>
    state === "needs_input" ? "#FBBF24" : state === "failed" ? "#F87171" : state === "review" ? "#4ADE80" : "#60A5FA";
  const muted = { type: "hierarchical" as const, style: "secondary" as const };
  const top = props.rows[0];
  const row = (item: AgentCardProps["rows"][number]) => (
    <HStack key={item.key} spacing={10}>
      <Image systemName={symbolFor(item.state)} size={18} color={colorFor(item.state)} />
      <VStack alignment="leading" spacing={2}>
        <Text modifiers={[font({ size: 15, weight: "semibold" }), foregroundStyle("#FFFFFF"), lineLimit(1)]}>{item.title}</Text>
        <Text modifiers={[font({ size: 12 }), foregroundStyle(muted), lineLimit(1)]}>{item.place}</Text>
      </VStack>
      <Spacer />
      <VStack alignment="trailing" spacing={2}>
        <Text modifiers={[font({ size: 12, weight: "semibold" }), foregroundStyle(colorFor(item.state))]}>{item.label}</Text>
        <Text date={new Date(item.since)} dateStyle="relative" modifiers={[font({ size: 11 }), foregroundStyle(muted), lineLimit(1)]} />
      </VStack>
    </HStack>
  );
  return {
    banner: (
      <VStack alignment="leading" spacing={10} modifiers={[padding({ all: 16 }), activityBackgroundTint("#0F1115")]}>
        <HStack spacing={6}>
          <Image systemName="sparkles" size={14} color="#A78BFA" />
          <Text modifiers={[font({ size: 14, weight: "bold" }), foregroundStyle("#FFFFFF")]}>Dragent</Text>
          <Spacer />
          <Text modifiers={[font({ size: 12, weight: "medium" }), foregroundStyle(muted), lineLimit(1)]}>{props.summary}</Text>
        </HStack>
        {props.rows.slice(0, 3).map(row)}
      </VStack>
    ),
    compactLeading: top ? (
      <Image systemName={symbolFor(top.state)} size={15} color={colorFor(top.state)} />
    ) : (
      <Image systemName="sparkles" size={15} color="#A78BFA" />
    ),
    compactTrailing: (
      <Text modifiers={[font({ size: 14, weight: "semibold" }), foregroundStyle(top ? colorFor(top.state) : "#FFFFFF")]}>
        {top ? (props.needsYou > 0 ? `${props.needsYou}` : `${props.working}`) : ""}
      </Text>
    ),
    minimal: top ? (
      <Image systemName={symbolFor(top.state)} size={14} color={colorFor(top.state)} />
    ) : (
      <Image systemName="sparkles" size={14} color="#A78BFA" />
    ),
    expandedLeading: (
      <HStack spacing={6} modifiers={[padding({ leading: 8 })]}>
        <Image systemName="sparkles" size={14} color="#A78BFA" />
        <Text modifiers={[font({ size: 14, weight: "bold" })]}>Dragent</Text>
      </HStack>
    ),
    expandedTrailing: (
      <Text modifiers={[font({ size: 12 }), foregroundStyle(muted), lineLimit(1), padding({ trailing: 8 })]}>{props.summary}</Text>
    ),
    expandedBottom: (
      <VStack alignment="leading" spacing={10} modifiers={[padding({ top: 6, leading: 8, trailing: 8, bottom: 4 })]}>
        {props.rows.slice(0, 2).map(row)}
      </VStack>
    ),
  };
});
