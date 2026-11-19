"use client";
import { useState } from "react";
import { Check, Copy } from "lucide-react";
export function CodeBlock({
  code,
  label = "Terminal",
}: {
  code: string;
  label?: string;
}) {
  const [status, setStatus] = useState<"idle" | "copied" | "error">("idle");
  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
      setStatus("copied");
    } catch {
      setStatus("error");
    }
  }
  return (
    <div className="doc-code">
      <div className="doc-code-label">
        <span>{label}</span>
        <button type="button" onClick={copy} aria-label="Copy command">
          {status === "copied" ? <Check size={13} /> : <Copy size={13} />}
          <span aria-live="polite">
            {status === "copied"
              ? "Copied"
              : status === "error"
                ? "Select and copy below"
                : "Copy"}
          </span>
        </button>
      </div>
      <pre tabIndex={0}>
        <code>{code}</code>
      </pre>
    </div>
  );
}
