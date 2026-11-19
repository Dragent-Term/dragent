import type { ReactNode } from "react";
import { StyleSheet, View, type ViewProps } from "react-native";
import { spacing } from "../theme/tokens";
import { PageHeader } from "./PageHeader";

export interface SectionProps extends ViewProps {
  title: string;
  description?: string;
  actions?: ReactNode;
}

export function Section({ title, description, actions, children, style, ...props }: SectionProps) {
  return (
    <View {...props} style={[styles.shell, style]}>
      <PageHeader title={title} subtitle={description} titleVariant="body" actions={actions} />
      {children}
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { gap: spacing.md },
});
