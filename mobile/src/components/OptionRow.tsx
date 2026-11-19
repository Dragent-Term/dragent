/**
 * A row of option buttons (selects, a due date, a toggle) that never
 * squeezes them: each shows its whole label, and when they don't all fit
 * the row scrolls sideways, running to the screen's edges so it's plain
 * there is more to swipe to.
 */
import { createContext, useContext, type ReactNode } from "react";
import { ScrollView, StyleSheet } from "react-native";
import { useResponsiveLayout } from "../layout/useResponsiveLayout";

const InOptionRow = createContext(false);

/** True inside an OptionRow: a select sizes to its label instead of sharing the width. */
export function useInOptionRow(): boolean {
  return useContext(InOptionRow);
}

export function OptionRow({ children, accessibilityLabel }: { children: ReactNode; accessibilityLabel?: string }) {
  const { gutter } = useResponsiveLayout();
  return (
    <InOptionRow.Provider value>
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        keyboardShouldPersistTaps="handled"
        accessibilityLabel={accessibilityLabel}
        style={{ marginHorizontal: -gutter, flexGrow: 0 }}
        contentContainerStyle={[styles.row, { paddingHorizontal: gutter }]}
      >
        {children}
      </ScrollView>
    </InOptionRow.Provider>
  );
}

const styles = StyleSheet.create({
  row: { flexDirection: "row", alignItems: "center", gap: 8 },
});
