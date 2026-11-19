import { Stack } from "expo-router";
import NotFound from "../+not-found";
import { PreviewProvider } from "../../src/preview/PreviewContext";

/** Preview actions exist only beneath this development-only route boundary. */
export default function PreviewLayout() {
  if (!__DEV__) return <NotFound />;
  return <PreviewProvider><Stack screenOptions={{ headerShown: false }} /></PreviewProvider>;
}
