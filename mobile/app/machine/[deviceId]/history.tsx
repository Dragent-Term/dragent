import { useLocalSearchParams } from "expo-router";
import { MachinePastSessionsScreen } from "../../../src/features/machines/MachinePastSessionsScreen";
import { singleParam } from "../../../src/machines/routes";

/** A folder's earlier agent conversations on the paired desktop. */
export default function MachinePastSessionsRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[]; folder?: string | string[]; title?: string | string[] }>();
  return (
    <MachinePastSessionsScreen
      deviceId={singleParam(params.deviceId)}
      folder={singleParam(params.folder)}
      title={singleParam(params.title) || "Past sessions"}
    />
  );
}
