import { useLocalSearchParams } from "expo-router";
import { MachineTabScreen } from "../../../../src/features/machines/MachineTabScreen";
import { singleParam } from "../../../../src/machines/routes";

export default function MachineTabRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[]; tabId?: string | string[] }>();
  const deviceId = singleParam(params.deviceId);
  const rawTab = singleParam(params.tabId) || undefined;
  const tabId = typeof rawTab === "string" && /^\d+$/.test(rawTab) ? Number(rawTab) : NaN;
  return (
    <MachineTabScreen
      key={`${deviceId}\u0000${typeof rawTab === "string" ? rawTab : ""}`}
      deviceId={deviceId}
      tabId={Number.isSafeInteger(tabId) && tabId >= 0 ? tabId : -1}
    />
  );
}
