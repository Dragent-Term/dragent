"use client";

import React, { useState, useEffect } from "react";
import { 
  CheckCircle2, 
  Copy, 
  ExternalLink, 
  Check, 
  Terminal, 
  LogOut 
} from "lucide-react";

interface SessionControlProps {
  session: {
    access_token?: string;
    refresh_token?: string;
    user_id?: string;
  };
  scheme: string;
  state: string;
}

export default function SessionControl({ session, scheme, state }: SessionControlProps) {
  const [copiedToken, setCopiedToken] = useState(false);
  const [copiedLink, setCopiedLink] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  
  const refreshToken = session.refresh_token || "";
  const userId = session.user_id || "unknown-user-id";
  
  // Format deep link
  const deepLinkUrl = `${scheme}://auth/callback?refresh_token=${encodeURIComponent(
    refreshToken
  )}&state=${encodeURIComponent(state)}&user_id=${encodeURIComponent(userId)}`;

  // Print system logs and trigger deep link automatically on mount
  useEffect(() => {
    const addLog = (msg: string, delay: number) => {
      setTimeout(() => {
        setLogs((prev) => [...prev, `[${new Date().toLocaleTimeString()}] ${msg}`]);
      }, delay);
    };

    addLog("Session established. Resolving credentials...", 100);
    addLog(`User identity verified: ${userId.substring(0, 8)}...`, 500);
    addLog(`Constructing handoff packet for ${scheme}://...`, 900);
    addLog("Attempting auto-launch redirect...", 1300);

    // Auto-launch deep link
    const triggerRedirect = setTimeout(() => {
      try {
        window.location.href = deepLinkUrl;
        setLogs((prev) => [
          ...prev,
          `[${new Date().toLocaleTimeString()}] 🚀 Launch callback URL sent to browser successfully.`
        ]);
      } catch (err) {
        setLogs((prev) => [
          ...prev,
          `[${new Date().toLocaleTimeString()}] ⚠️ Auto-launch redirect encountered browser restriction.`
        ]);
      }
    }, 1500);

    return () => clearTimeout(triggerRedirect);
  }, [deepLinkUrl, scheme, userId]);

  // Copy token to clipboard
  const handleCopyToken = () => {
    navigator.clipboard.writeText(refreshToken);
    setCopiedToken(true);
    setTimeout(() => setCopiedToken(false), 2000);
  };

  // Copy link to clipboard
  const handleCopyLink = () => {
    navigator.clipboard.writeText(deepLinkUrl);
    setCopiedLink(true);
    setTimeout(() => setCopiedLink(false), 2000);
  };

  const triggerDeepLink = () => {
    window.location.href = deepLinkUrl;
  };

  return (
    <div className="space-y-6 text-left">
      {/* Success banner */}
      <div className="flex items-center gap-4 p-4 rounded-xl bg-brand-emerald/10 border border-brand-emerald/20">
        <CheckCircle2 className="w-8 h-8 text-brand-emerald shrink-0 animate-bounce" />
        <div>
          <h2 className="text-md font-bold text-white">Sign In Successful!</h2>
          <p className="text-xs text-gray-400">Your session is authenticated and ready to link.</p>
        </div>
      </div>

      {/* Connection options */}
      <div className="space-y-3">
        <button
          onClick={triggerDeepLink}
          className="w-full py-3.5 bg-brand-cyan hover:bg-brand-cyan/90 active:bg-brand-cyan/85 text-white font-semibold rounded-xl transition-colors flex items-center justify-center gap-2 text-sm shadow-lg shadow-brand-cyan/20"
        >
          <span>Launch Desktop App</span>
          <ExternalLink className="w-4 h-4" />
        </button>

        <div className="grid grid-cols-2 gap-3">
          <button
            onClick={handleCopyLink}
            className="py-2.5 bg-[#141414] hover:bg-[#1c1c1f] text-gray-300 hover:text-white border border-white/5 rounded-lg transition-all flex items-center justify-center gap-1.5 text-xs"
          >
            {copiedLink ? <Check className="w-4 h-4 text-brand-emerald" /> : <Copy className="w-4 h-4" />}
            <span>{copiedLink ? "Copied Link!" : "Copy Link"}</span>
          </button>
          
          <button
            onClick={handleCopyToken}
            className="py-2.5 bg-[#141414] hover:bg-[#1c1c1f] text-gray-300 hover:text-white border border-white/5 rounded-lg transition-all flex items-center justify-center gap-1.5 text-xs"
          >
            {copiedToken ? <Check className="w-4 h-4 text-brand-emerald" /> : <Copy className="w-4 h-4" />}
            <span>{copiedToken ? "Copied Token!" : "Copy Token"}</span>
          </button>
        </div>
      </div>

      {/* Session details */}
      <div className="bg-[#101010] border border-white/5 rounded-xl p-4 space-y-3">
        <div className="text-[10px] font-bold text-gray-500 uppercase tracking-widest">
          Session Metadata
        </div>
        <div className="space-y-2 text-xs">
          <div className="flex justify-between">
            <span className="text-gray-500">Identity:</span>
            <span className="text-gray-300 font-mono select-all">{userId}</span>
          </div>
          <div className="flex justify-between">
            <span className="text-gray-500">Handoff State:</span>
            <span className="text-gray-300 font-mono">{state.substring(0, 16)}...</span>
          </div>
          <div className="flex justify-between">
            <span className="text-gray-500">Token Type:</span>
            <span className="text-gray-300">Dragent Session JWT</span>
          </div>
        </div>
      </div>

      {/* Simulator terminal logs */}
      <div className="bg-[#080808] border border-white/5 rounded-xl p-4 font-mono text-xs">
        <div className="flex items-center gap-2 mb-2 pb-2 border-b border-white/5 text-gray-500 text-[10px] uppercase font-bold tracking-wider">
          <Terminal className="w-3.5 h-3.5 text-brand-cyan" />
          <span>Handshake Logs</span>
        </div>
        <div className="space-y-1.5 max-h-36 overflow-y-auto text-[11px] text-gray-400">
          {logs.map((log, index) => (
            <div key={index} className="whitespace-pre-wrap leading-relaxed">
              {log}
            </div>
          ))}
        </div>
      </div>

      {/* Back option */}
      <div className="text-center">
        <button
          onClick={() => window.location.reload()}
          className="text-xs text-gray-500 hover:text-gray-400 flex items-center justify-center gap-1 mx-auto transition-colors"
        >
          <LogOut className="w-3 h-3" />
          <span>Return to login</span>
        </button>
      </div>
    </div>
  );
}
