"use client";
import { useState } from "react";
import Link from "next/link";
import { ChevronDown, ArrowUpRight } from "lucide-react";
import type { DocNavigation } from "./catalog";
export function DocsNavigation({
  groups,
  active,
}: {
  groups: DocNavigation;
  active?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <aside className="docs-sidebar">
      <button
        type="button"
        className="docs-mobile-menu"
        aria-expanded={open}
        aria-controls="docs-navigation"
        onClick={() => setOpen(!open)}
      >
        Browse documentation <ChevronDown size={15} />
      </button>
      <nav
        aria-label="Documentation navigation"
        id="docs-navigation"
        className={open ? "docs-navigation open" : "docs-navigation"}
      >
        <Link
          href="/docs"
          className="docs-overview-link"
          aria-current={!active ? "page" : undefined}
        >
          Documentation overview
        </Link>
        {groups.map((group) => (
          <section key={group.label}>
            <h2>{group.label}</h2>
            <ul>
              {group.sections.map((item) => (
                <li key={item.id}>
                  <Link
                    href={`/docs/${item.id}`}
                    aria-current={active === item.id ? "page" : undefined}
                    onClick={() => setOpen(false)}
                  >
                    {item.label}
                  </Link>
                </li>
              ))}
            </ul>
          </section>
        ))}
        <Link href="/download" className="docs-sidebar-download">
          Download Dragent <ArrowUpRight size={13} />
        </Link>
      </nav>
    </aside>
  );
}
