"use client";

import Link from "next/link";
import { useEffect, useRef, useState } from "react";
import { ArrowUpRight, ChevronDown, Menu, X } from "lucide-react";
import { Logo } from "./Logo";
import { site } from "@/lib/site";

const products = [
  {
    name: "Agent coordination",
    description: "Give a team one goal. Follow every task.",
    href: "/#coordination",
  },
  {
    name: "Workspaces & worktrees",
    description: "Keep parallel work on separate branches.",
    href: "/#workspaces",
  },
  {
    name: "Remote development",
    description: "Bring your own machine over SSH.",
    href: "/#remote",
  },
  {
    name: "Code review",
    description: "Read the diff. Discuss it. Ship with context.",
    href: "/#reviews",
  },
];

export function Nav() {
  const [mobile, setMobile] = useState(false);
  const header = useRef<HTMLElement>(null);
  useEffect(() => {
    const close = (event: PointerEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent && event.key !== "Escape") return;
      const outside =
        event instanceof PointerEvent &&
        !header.current?.contains(event.target as Node);
      if (outside || event instanceof KeyboardEvent) {
        setMobile(false);
        header.current
          ?.querySelectorAll("details[open]")
          .forEach((item) => item.removeAttribute("open"));
      }
    };
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", close);
    return () => {
      document.removeEventListener("pointerdown", close);
      document.removeEventListener("keydown", close);
    };
  }, []);
  return (
    <header ref={header} className="site-header">
      <a className="skip-link" href="#main-content">
        Skip to content
      </a>
      <div className="site-nav container-wide">
        <Link href="/" aria-label="Dragent home" className="brand-link">
          <Logo />
        </Link>
        <nav aria-label="Main navigation" className="desktop-nav">
          <details className="product-menu">
            <summary>
              Product <ChevronDown size={13} aria-hidden="true" />
            </summary>
            <div className="product-menu-panel">
              {products.map((product) => (
                <Link
                  key={product.href}
                  href={product.href}
                  onClick={(event) =>
                    event.currentTarget
                      .closest("details")
                      ?.removeAttribute("open")
                  }
                >
                  <span>{product.name}</span>
                  <small>{product.description}</small>
                </Link>
              ))}
            </div>
          </details>
          <Link href="/pricing">Pricing</Link>
          <Link href="/docs">Documentation</Link>
          <Link href="/docs/quickstart">
            Get started <ArrowUpRight size={12} aria-hidden="true" />
          </Link>
        </nav>
        <div className="nav-actions">
          <Link href={site.appUrl} className="nav-sign-in">
            Sign in
          </Link>
          <Link className="button button-small" href="/download">
            Download <ArrowUpRight size={13} aria-hidden="true" />
          </Link>
          <button
            type="button"
            className="mobile-toggle icon-button"
            aria-label={mobile ? "Close navigation" : "Open navigation"}
            aria-expanded={mobile}
            aria-controls="mobile-navigation"
            onClick={() => setMobile(!mobile)}
          >
            {mobile ? <X size={21} /> : <Menu size={21} />}
          </button>
        </div>
      </div>
      <nav
        id="mobile-navigation"
        hidden={!mobile}
        className="mobile-navigation"
        aria-label="Mobile navigation"
      >
        {[
          ...products,
          { name: "Pricing", href: "/pricing" },
          { name: "Documentation", href: "/docs" },
          { name: "Sign in", href: site.appUrl },
        ].map((item) => (
          <Link
            onClick={() => setMobile(false)}
            key={item.href}
            href={item.href}
          >
            {item.name}
            <ArrowUpRight size={15} aria-hidden="true" />
          </Link>
        ))}
      </nav>
    </header>
  );
}
