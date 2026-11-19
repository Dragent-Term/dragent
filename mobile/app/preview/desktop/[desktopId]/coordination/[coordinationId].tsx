import { CoordinationDetailScreen } from "../../../../../src/features/coordination/CoordinationDetailScreen";
import { usePreviewRouteParams } from "../../../../../src/preview/usePreviewRouteParams";
export default function Route() {
  const { desktopId, coordinationId } = usePreviewRouteParams();
  return <CoordinationDetailScreen key={coordinationId} desktopId={desktopId} coordinationId={coordinationId} />;
}
