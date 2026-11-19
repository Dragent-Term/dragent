import { createContext, type ReactNode } from "react";

export interface LayoutViewportProviderProps {
  width: number;
  children: ReactNode;
}

export const LayoutViewportContext = createContext<number | null>(null);

/** Available main-pane width after the shell allocates navigation and insets. */
export function LayoutViewportProvider({ width, children }: LayoutViewportProviderProps) {
  return <LayoutViewportContext.Provider value={width}>{children}</LayoutViewportContext.Provider>;
}
