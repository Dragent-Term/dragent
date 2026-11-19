import type { ReactNode } from "react";
/** Content remains visible during server rendering and without JavaScript. */
export function Reveal({
  children,
  className = "",
}: {
  children: ReactNode;
  delay?: number;
  className?: string;
}) {
  return <div className={className}>{children}</div>;
}
