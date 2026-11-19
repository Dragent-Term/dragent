import { createContext, useContext, useMemo, useSyncExternalStore, type ReactNode } from "react";
import { PreviewStore } from "./PreviewStore";
import type { PreviewValue } from "./types";

export { selectPreviewProject, selectPreviewWorkspace, selectPreviewTab } from "./PreviewStore";

const PreviewContext = createContext<PreviewStore | null>(null);

export function PreviewProvider({ children }: { children: ReactNode }) {
  const store = useMemo(() => new PreviewStore(), []);
  return <PreviewContext value={store}>{children}</PreviewContext>;
}

export function usePreview(): PreviewValue {
  const store = useContext(PreviewContext);
  if (!store) throw new Error("usePreview requires PreviewProvider");
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  return {
    ...snapshot, getWorkspace: store.getWorkspace, getTab: store.getTab, getWriters: store.getWriters,
    createWorkspace: store.createWorkspace, createTab: store.createTab, toggleTask: store.toggleTask, reset: store.reset,
    navigateBrowser: store.navigateBrowser, goBrowser: store.goBrowser, reloadBrowser: store.reloadBrowser,
    sendCoordinationMessage: store.sendCoordinationMessage, stopCoordination: store.stopCoordination,
    configureCoordination: store.configureCoordination, toggleAutomation: store.toggleAutomation, runAutomation: store.runAutomation,
    setReviewFileReviewed: store.setReviewFileReviewed, addReviewNote: store.addReviewNote,
    markPullRequestFileViewed: store.markPullRequestFileViewed, addPullRequestNote: store.addPullRequestNote,
    markTabSeen: store.markTabSeen,
    createPullRequest: store.createPullRequest,
  };
}
