import { TERMINAL_WASM_BASE64, TERMINAL_WASM_GLUE, TERMINAL_BROWSER_SOURCE } from './wasmAsset.generated';
import { RUNTIME_LIMITS } from './types';

export interface TerminalDocumentOptions {
  /**
   * 'auto' (default) draws with WebGL2 and falls back to the 2D canvas when it
   * is missing, lost or fails; '2d' and 'gpu' pin one, for comparisons.
   */
  renderer?: 'auto' | '2d' | 'gpu';
}

/**
 * Isolated local document only: no remote resources, credentials, sockets or
 * URL fetching. Host text arrives as JSON postMessages; the renderer never
 * dials the gateway. Content-Security-Policy blocks network, base and frames.
 */
export function terminalDocument(id: string, options: TerminalDocumentOptions = {}): string {
  if (!/^[a-zA-Z0-9_-]{1,128}$/u.test(id)) throw new Error('Invalid terminal bridge identifier');
  const renderer = options.renderer === '2d' || options.renderer === 'gpu' ? options.renderer : 'auto';
  const raw = `${TERMINAL_WASM_GLUE}\nconst binary=Uint8Array.from(atob(${JSON.stringify(TERMINAL_WASM_BASE64)}),c=>c.charCodeAt(0));\ntry{initSync({module:binary});(${TERMINAL_BROWSER_SOURCE})(Terminal,${JSON.stringify(id)},${JSON.stringify(RUNTIME_LIMITS)},${JSON.stringify({ renderer })});}catch{const message=JSON.stringify({v:1,id:${JSON.stringify(id)},type:'error'});if(window.ReactNativeWebView)window.ReactNativeWebView.postMessage(message);else window.parent.postMessage(message,'*');}`;
  // Generated assets are trusted, but never let them break out of the inline script.
  const script = raw.replace(/<\/script/gi, '<\\/script');
  return `<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'unsafe-inline'; connect-src 'none'; img-src data:; base-uri 'none'; form-action 'none'"><style>html,body{margin:0;width:100%;height:100%;overflow:hidden;overscroll-behavior:none;background:transparent}#latest{display:none;position:absolute;z-index:1;touch-action:manipulation;right:12px;bottom:12px;min-height:44px;padding:8px 16px;border:1px solid #777;border-radius:22px;background:#222;color:#fff;font:14px system-ui}#screen{touch-action:none;display:block;width:100%;height:100%;outline:none}#keys{position:absolute;left:0;bottom:0;width:1px;height:1px;opacity:.01;font-size:16px;resize:none;border:0;padding:0}#status{position:absolute;inset:0;display:none;align-items:center;justify-content:center;padding:20px;font:14px/1.5 system-ui;text-align:center}#sel{position:absolute;inset:0;pointer-events:none;z-index:1}#menu{display:none;position:absolute;z-index:3;left:50%;transform:translateX(-50%);gap:2px;padding:4px;border-radius:12px;background:#1d2026;border:1px solid #3a3f48;box-shadow:0 6px 18px rgba(0,0,0,.35)}#menu button{touch-action:manipulation;min-height:40px;padding:8px 16px;border:0;border-radius:8px;background:transparent;color:#fff;font:600 15px system-ui}#menu button:active{background:#333a44}#readable{position:absolute;left:-10000px;top:0;white-space:pre;content-visibility:auto;contain-intrinsic-size:auto 1px auto 1px}</style></head><body><canvas id="screen" role="button" tabindex="0" aria-label="Terminal screen. Tap a link to open it; touch and hold to select and copy."></canvas><div id="sel" aria-hidden="true"></div><div id="menu" role="menu"><button id="menu-copy" role="menuitem">Copy</button><button id="menu-open" role="menuitem">Open link</button></div><button id="latest" aria-label="Scroll to latest terminal output">↓ Latest</button><div id="status" role="status"></div><textarea id="keys" aria-label="Live terminal input" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false" disabled></textarea><pre id="readable" role="log" aria-label="Terminal screen text"></pre><script type="module">${script}</script></body></html>`;
}
