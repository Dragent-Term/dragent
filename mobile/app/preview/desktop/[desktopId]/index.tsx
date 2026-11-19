import { MachineScreen } from "../../../../src/features/preview/MachineScreen";
import { usePreviewRouteParams } from "../../../../src/preview/usePreviewRouteParams";
export default function DesktopRoute() {
  const { desktopId } = usePreviewRouteParams();
  return <MachineScreen key={desktopId} desktopId={desktopId} />;
}
