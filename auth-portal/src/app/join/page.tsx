"use client";

import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { ShieldAlert, Loader2 } from "lucide-react";
import WasmViewer from "@/components/WasmViewer";

// The shared-session join page. The viewing experience — navbar, presence stack,
// terminal, request-edit, "Open Dragent" — is the Dragent desktop app compiled
// to WebAssembly (`WasmViewer`), which opens its own gateway WebSocket and reads
// the session credentials straight from this page's URL (`?session=&secret=`).
// React only guards for a complete invite link and mounts the wasm.

export default function JoinPage() {
  return (
    <Suspense
      fallback={
        <div className="min-h-screen flex items-center justify-center bg-[#0a0b0e]">
          <Loader2 className="w-6 h-6 animate-spin text-gray-500" />
        </div>
      }
    >
      <JoinContent />
    </Suspense>
  );
}

function JoinContent() {
  const searchParams = useSearchParams();
  const session = searchParams.get("session");
  const secret = searchParams.get("secret");

  if (!session || !secret) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-[#0a0b0e] p-6">
        <div className="flex items-start gap-3 p-4 rounded-lg bg-rose-500/10 border border-rose-500/20 text-sm text-rose-300 max-w-md">
          <ShieldAlert className="w-5 h-5 mt-0.5 shrink-0" />
          <div>
            This invite link is incomplete or has expired. Ask the host to share a
            fresh link.
          </div>
        </div>
      </div>
    );
  }

  return <WasmViewer />;
}
