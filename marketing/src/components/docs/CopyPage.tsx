"use client";
import { useState } from "react";
import { Check, Copy } from "lucide-react";
export function CopyPage() {
  const [status, setStatus] = useState("Copy page");
  async function copy() {
    const text =
      document.querySelector<HTMLElement>("[data-doc-body]")?.innerText;
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setStatus("Copied");
    } catch {
      setStatus("Select text to copy");
    }
  }
  return (
    <button className="copy-page" type="button" onClick={copy}>
      {status === "Copied" ? <Check size={13} /> : <Copy size={13} />}
      <span aria-live="polite">{status}</span>
    </button>
  );
}
