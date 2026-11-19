import type { ReactNode } from "react";
import { headingId, nodeText } from "./text";
export function P({ children }: { children: ReactNode }) {
  return <p className="doc-paragraph">{children}</p>;
}
export function H3({ id, children }: { id?: string; children: ReactNode }) {
  const anchor = id ?? headingId(nodeText(children));
  return (
    <h2 id={anchor} className="doc-heading">
      <a href={`#${anchor}`}>
        {children}
        <span aria-hidden="true">#</span>
      </a>
    </h2>
  );
}
export function UL({ children }: { children: ReactNode }) {
  return <ul className="doc-list">{children}</ul>;
}
export function LI({ children }: { children: ReactNode }) {
  return <li>{children}</li>;
}
export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="doc-kbd">{children}</kbd>;
}
export function C({ children }: { children: ReactNode }) {
  return <code className="doc-inline-code">{children}</code>;
}
export function Note({ children }: { children: ReactNode }) {
  return (
    <aside className="doc-note">
      <span className="doc-note-label">Good to know</span>
      <div>{children}</div>
    </aside>
  );
}
export function Table({ head, rows }: { head: string[]; rows: ReactNode[][] }) {
  return (
    <div className="doc-table-wrap">
      <table className="doc-table">
        <thead>
          <tr>
            {head.map((h) => (
              <th scope="col" key={h}>
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, i) => (
            <tr key={i}>
              {row.map((cell, j) => (
                <td key={j}>{cell}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
