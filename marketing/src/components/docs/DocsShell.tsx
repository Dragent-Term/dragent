import Link from "next/link";
import {
  ArrowLeft,
  ArrowRight,
  ArrowUpRight,
  ChevronRight,
} from "lucide-react";
import { Nav } from "../Nav";
import { Footer } from "../Footer";
import { DocsNavigation } from "./DocsNavigation";
import { DocsSearch } from "./DocsSearch";
import { DocsToc } from "./DocsToc";
import { CopyPage } from "./CopyPage";
import {
  allDocs,
  docGroup,
  headings,
  navigation,
  resolveDocLinks,
  searchEntries,
} from "./catalog";
import { nodeText } from "./text";
import type { DocSection } from "./content";
import { site } from "@/lib/site";

export function DocsShell({
  current,
  children,
}: {
  current?: DocSection;
  children?: React.ReactNode;
}) {
  const index = current
    ? allDocs.findIndex((doc) => doc.id === current.id)
    : -1;
  const previous = allDocs[index - 1],
    next = allDocs[index + 1];
  const reading = current
    ? Math.max(1, Math.ceil(nodeText(current.body).split(/\s+/).length / 220))
    : 0;
  return (
    <>
      <Nav />
      <main id="main-content" className="docs-main">
        <div className="docs-topline">
          <div>
            <Link href="/docs">Documentation</Link>
            {current ? (
              <>
                <ChevronRight size={12} aria-hidden="true" />
                <span>{current.label}</span>
              </>
            ) : null}
          </div>
          <DocsSearch entries={searchEntries} />
        </div>
        <div className="docs-layout">
          <DocsNavigation groups={navigation} active={current?.id} />
          <article className="docs-article">
            {current ? (
              <>
                <header className="doc-title">
                  <div className="doc-title-top">
                    <p className="eyebrow">{docGroup(current)}</p>
                    <CopyPage />
                  </div>
                  <h1>{current.title}</h1>
                  {current.summary ? (
                    <p className="doc-description">{current.summary}</p>
                  ) : null}
                  <p className="doc-reading">
                    {reading} min read <span aria-hidden="true">·</span> Dragent
                    guide
                  </p>
                </header>
                <div className="doc-body" data-doc-body>
                  {resolveDocLinks(current.body)}
                </div>
                <div className="doc-feedback">
                  <span>Something unclear?</span>
                  <a
                    href={`mailto:${site.contactEmail}?subject=${encodeURIComponent(`Docs: ${current.title}`)}`}
                  >
                    Help us improve this page{" "}
                    <ArrowUpRight size={12} aria-hidden="true" />
                  </a>
                </div>
                <nav
                  className="doc-pagination"
                  aria-label="Documentation pagination"
                >
                  {previous ? (
                    <Link href={`/docs/${previous.id}`}>
                      <small>
                        <ArrowLeft size={12} />
                        Previous
                      </small>
                      <strong>{previous.label}</strong>
                    </Link>
                  ) : (
                    <span />
                  )}
                  {next ? (
                    <Link href={`/docs/${next.id}`}>
                      <small>
                        Next
                        <ArrowRight size={12} />
                      </small>
                      <strong>{next.label}</strong>
                    </Link>
                  ) : (
                    <span />
                  )}
                </nav>
              </>
            ) : (
              children
            )}
          </article>
          <aside className="docs-right-rail">
            {current ? (
              <DocsToc items={headings(current.body)} />
            ) : (
              <div className="docs-toc">
                <p>A good place to start</p>
                <Link href="/docs/quickstart">Your first workspace</Link>
                <Link href="/docs/concepts">The Dragent model</Link>
                <Link href="/docs/troubleshooting">Troubleshooting</Link>
              </div>
            )}
          </aside>
        </div>
      </main>
      <Footer />
    </>
  );
}
