"use client";
import { useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { ArrowRight, FileText, Search, X } from "lucide-react";
import type { SearchEntry } from "./catalog";

export function DocsSearch({ entries }: { entries: SearchEntry[] }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const router = useRouter();
  const words = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
  const results = (
    words.length
      ? entries
          .filter((entry) =>
            words.every((word) =>
              `${entry.title} ${entry.description} ${entry.keywords}`
                .toLowerCase()
                .includes(word),
            ),
          )
          .sort(
            (a, b) =>
              Number(b.title.toLowerCase().includes(query.toLowerCase())) -
              Number(a.title.toLowerCase().includes(query.toLowerCase())),
          )
      : entries.filter((entry) =>
          [
            "quickstart",
            "workspaces",
            "coordination",
            "remote-ssh",
            "code-review",
            "cli",
          ].includes(entry.id),
        )
  ).slice(0, 8);
  function open() {
    if (!dialog.current?.open) dialog.current?.showModal();
    requestAnimationFrame(() => input.current?.focus());
  }
  function close() {
    dialog.current?.close();
    trigger.current?.focus();
  }
  function choose(id: string) {
    close();
    router.push(`/docs/${id}`);
  }
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        if (dialog.current?.open) dialog.current.close();
        else {
          dialog.current?.showModal();
          input.current?.focus();
        }
      }
    };
    document.addEventListener("keydown", key);
    return () => document.removeEventListener("keydown", key);
  }, []);
  return (
    <>
      <button
        type="button"
        ref={trigger}
        className="docs-search-trigger"
        aria-haspopup="dialog"
        aria-controls="documentation-search"
        onClick={open}
      >
        <Search size={15} aria-hidden="true" />
        <span>Search documentation…</span>
        <kbd>⌘ / Ctrl K</kbd>
      </button>
      <dialog
        id="documentation-search"
        ref={dialog}
        className="docs-search-dialog"
        aria-labelledby="docs-search-title"
        onClick={(event) => {
          if (event.target === event.currentTarget) close();
        }}
        onClose={() => {
          setSelected(0);
        }}
      >
        <div className="docs-search-box">
          <div className="docs-search-input">
            <Search size={18} aria-hidden="true" />
            <label
              htmlFor="docs-search"
              id="docs-search-title"
              className="sr-only"
            >
              Search documentation
            </label>
            <input
              id="docs-search"
              name="q"
              type="search"
              spellCheck={false}
              ref={input}
              value={query}
              placeholder="Search workspaces, SSH, or reviews…"
              autoComplete="off"
              onChange={(event) => {
                setQuery(event.target.value);
                setSelected(0);
              }}
              role="combobox"
              aria-autocomplete="list"
              aria-expanded="true"
              aria-controls="docs-search-results"
              aria-activedescendant={
                results.length
                  ? `search-${results[Math.min(selected, results.length - 1)].id}`
                  : undefined
              }
              onKeyDown={(event) => {
                if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                  event.preventDefault();
                  setSelected((value) =>
                    results.length
                      ? (value +
                          (event.key === "ArrowDown"
                            ? 1
                            : results.length - 1)) %
                        results.length
                      : 0,
                  );
                }
                if (event.key === "Enter" && results[selected]) {
                  event.preventDefault();
                  choose(results[selected].id);
                }
              }}
            />
            <button
              className="icon-button"
              type="button"
              onClick={close}
              aria-label="Close search"
            >
              <X size={18} />
            </button>
          </div>
          <div className="search-section-label">
            {query ? "Search results" : "Start with a workflow"}
          </div>
          <p className="sr-only" role="status">
            {results.length} matching pages
          </p>
          <div
            id="docs-search-results"
            role="listbox"
            aria-label="Documentation results"
          >
            {results.map((entry, index) => (
              <button
                key={entry.id}
                type="button"
                id={`search-${entry.id}`}
                role="option"
                aria-selected={selected === index}
                className="search-result"
                onMouseEnter={() => setSelected(index)}
                onClick={() => choose(entry.id)}
              >
                <FileText size={16} aria-hidden="true" />
                <span>
                  <strong>{entry.title}</strong>
                  <small>{entry.description}</small>
                </span>
                <ArrowRight size={14} aria-hidden="true" />
              </button>
            ))}
            {!results.length ? (
              <div className="search-empty">
                <strong>No matching pages</strong>
                <p>Try “SSH”, “worktree”, “account”, or “review”.</p>
              </div>
            ) : null}
          </div>
          <div className="search-footer">
            <span>↑ ↓ to navigate</span>
            <span>Enter to open</span>
            <span>Esc to close</span>
          </div>
        </div>
      </dialog>
    </>
  );
}
