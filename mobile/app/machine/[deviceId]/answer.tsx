import { useLocalSearchParams } from "expo-router";
import { MachineAnswerScreen } from "../../../src/features/machines/MachineAnswerScreen";
import { singleParam } from "../../../src/machines/routes";

/** Allow or Deny from a notification: answered through the desktop. */
export default function MachineAnswerRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[]; pane?: string | string[]; choice?: string | string[] }>();
  return <MachineAnswerScreen deviceId={singleParam(params.deviceId)} pane={singleParam(params.pane)} choice={singleParam(params.choice)} />;
}
