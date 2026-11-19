import { AppText } from "../components/AppText";
import { CompanionHeader, type CompanionHeaderProps } from "../components/CompanionHeader";

export type PreviewHeaderProps = Omit<CompanionHeaderProps, "status">;

/** Preview labeling stays separate from the shared companion navigation. */
export function PreviewHeader(props: PreviewHeaderProps) {
  return <CompanionHeader {...props} status={
    <AppText variant="micro" tone="textMuted" accessibilityLabel="Local preview using sample data">Preview</AppText>
  } />;
}
