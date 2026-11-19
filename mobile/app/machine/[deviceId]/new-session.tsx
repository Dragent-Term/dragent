import { useLocalSearchParams } from "expo-router";
import { MachineNewSessionScreen } from "../../../src/features/machines/MachineNewSessionScreen";
import { singleParam } from "../../../src/machines/routes";

/** A new agent session, started on the paired desktop. */
export default function MachineNewSessionRoute() {
  const params = useLocalSearchParams<{ deviceId?: string | string[]; project?: string | string[]; workspace?: string | string[] }>();
  const project = singleParam(params.project);
  const workspace = singleParam(params.workspace);
  return (
    <MachineNewSessionScreen
      deviceId={singleParam(params.deviceId)}
      {...(project ? { project } : {})}
      {...(workspace ? { workspace } : {})}
    />
  );
}
