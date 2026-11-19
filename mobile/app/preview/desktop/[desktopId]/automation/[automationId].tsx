import { AutomationDetailScreen } from "../../../../../src/features/automations/AutomationDetailScreen";
import { usePreviewRouteParams } from "../../../../../src/preview/usePreviewRouteParams";
export default function Route() {
  const { desktopId, automationId } = usePreviewRouteParams();
  return <AutomationDetailScreen key={automationId} desktopId={desktopId} automationId={automationId} />;
}
