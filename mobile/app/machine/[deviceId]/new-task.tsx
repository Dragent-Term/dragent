import { useLocalSearchParams } from "expo-router";
import { MachineNewTaskScreen } from "../../../src/features/machines/MachineNewTaskScreen";
import { singleParam } from "../../../src/machines/routes";

/** A new task in the organisation, created through the paired desktop. */
export default function MachineNewTaskRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[] }>();
  return <MachineNewTaskScreen deviceId={singleParam(params.deviceId)} />;
}
