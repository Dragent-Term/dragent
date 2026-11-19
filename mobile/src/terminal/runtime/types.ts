import type { Ref } from 'react';
import type { TerminalAltScreen } from '../types';
import type { TerminalRuntimeStore } from './TerminalRuntimeStore';
import type { TerminalPalette } from '../../theme/types';

/**
 * `rawBytes` / `rawChunks` bound the store's rolling raw window (decoded bytes,
 * chunks). The window must fit one bridge packet (base64 grows by 4/3) with
 * room for a 256 KiB seed plus a burst behind it.
 */
export const RUNTIME_LIMITS = Object.freeze({ rawBytes: 1024 * 1024, rawChunks: 2048, bridgeCharacters: 3 * 1024 * 1024, rows: 240, cols: 512, cells: 65_536, inputBytes: 65_536 });

/** How long the bridge waits for the local renderer to acknowledge a frame. */
export const BRIDGE_ACK_TIMEOUT_MS = 12_000;

/**
 * Resync copy shown when the host grid or raw byte stream cannot be rendered.
 * Every message names the recovery action so readers know what to do next.
 */
export const GRID_RESYNC_MESSAGE =
  'The host terminal grid exceeds this device’s rendering limits. Reconnect to resynchronize.';
export const RAW_RESYNC_MESSAGE =
  'Terminal bytes were missed or exceeded the buffer. Reconnect to resynchronize; no partial VT stream was rendered.';
export const RENDERER_ERROR_MESSAGE =
  'The local terminal renderer stopped responding. Restart it to resynchronize.';
export type RuntimePresentation = 'native' | 'raw' | 'alternate' | 'resync';
export interface RuntimeSnapshot {
  readonly epoch: number;
  readonly revision: number;
  readonly presentation: RuntimePresentation;
  readonly reason: string | null;
}
export interface RawChunk { readonly seq: number; readonly data: string }
export interface RuntimeScreenModes {
  readonly alternateScreen: boolean;
  readonly mouseReporting: boolean;
  readonly sgrMouse: boolean;
  readonly bracketedPaste: boolean;
}
export interface RuntimeFrame extends RuntimeSnapshot {
  readonly rawReset: boolean;
  readonly rawChunks: readonly RawChunk[];
  readonly lastRawSeq: number;
  readonly rows: number;
  readonly cols: number;
  readonly grid: TerminalAltScreen | null;
  readonly screenModes?: RuntimeScreenModes;
}
export interface RuntimeEnvironment {
  connected: boolean;
  canWrite: boolean;
  canResize: boolean;
  /** While true the renderer keeps its fit and posts no resize. */
  holdFit: boolean;
  palette: TerminalPalette;
}
export interface RuntimePacket {
  v: 1;
  id: string;
  type: 'state';
  epoch: number;
  seq: number;
  frame: RuntimeFrame;
  environment: RuntimeEnvironment;
}
/** Not state: it carries no sequence, so it can go out between acknowledgements. */
export interface RuntimeBlurPacket {
  v: 1;
  id: string;
  type: 'blur';
}
export interface RuntimeCallbacks {
  send: (packet: RuntimePacket | RuntimeBlurPacket) => void;
  onInput: (text: string) => boolean;
  onResize: (rows: number, cols: number) => boolean;
  onMetrics?: (metrics: KeyboardAvoidanceMetrics) => void;
  onError?: (message: string) => void;
  /** The person chose Copy on text they selected. */
  onCopy?: (text: string) => void;
  /** The person tapped a web link (http or https only). */
  onLink?: (url: string) => void;
}

/** Selected text may be the whole screen several times over, never more. */
export const COPY_LIMIT_CHARS = 60_000;
const LINK_LIMIT_CHARS = 4_096;

/** Only a plain web link leaves the terminal, never another scheme. */
export function openableLink(value: unknown): string | null {
  if (typeof value !== 'string' || value.length === 0 || value.length > LINK_LIMIT_CHARS) return null;
  let url: URL;
  try { url = new URL(value); } catch { return null; }
  if (url.protocol !== 'https:' && url.protocol !== 'http:') return null;
  if (url.username || url.password) return null;
  return url.toString();
}
/** What the native side may ask of the renderer's hidden input. */
export interface TerminalInputControl {
  blurInput(): void;
}
/** Renderer props stay in one place so native and web exports cannot drift. */
export interface WasmTerminalViewProps {
  store: TerminalRuntimeStore;
  connected: boolean;
  canWrite: boolean;
  canResize: boolean;
  /** While true the renderer keeps its fit and posts no resize. */
  holdFit: boolean;
  onInput: (text: string) => boolean;
  onResize: (rows: number, cols: number) => boolean;
  onMetrics?: (metrics: KeyboardAvoidanceMetrics) => void;
  inputControl?: Ref<TerminalInputControl>;
}
/** What the renderer reports for keyboard-avoidance lift math. */
export interface KeyboardAvoidanceMetrics {
  rows: number;
  cursorRow: number;
  altScreen: boolean;
  /** Drawn px per row, fit scale included; absent before first measure. */
  rowPitch?: number;
  /** Drawn px from the frame top to the first grid row. */
  top?: number;
}
/** Renderer metrics are telemetry, so malformed payloads are dropped. */
export function validKeyboardAvoidanceMetrics(value: unknown): value is KeyboardAvoidanceMetrics {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const candidate = value as Record<string, unknown>;
  if (
    !Number.isSafeInteger(candidate.rows) ||
    (candidate.rows as number) < 1 ||
    (candidate.rows as number) > RUNTIME_LIMITS.rows ||
    !Number.isSafeInteger(candidate.cursorRow) ||
    (candidate.cursorRow as number) < 0 ||
    candidate.altScreen !== true && candidate.altScreen !== false
  ) return false;
  if (candidate.rowPitch !== undefined && (!Number.isFinite(candidate.rowPitch) || (candidate.rowPitch as number) <= 0)) return false;
  if (candidate.top !== undefined && (!Number.isFinite(candidate.top) || (candidate.top as number) < 0)) return false;
  return true;
}
/** Clamp the cursor into the grid; the renderer already clamps, this is belt and braces. */
export function sanitizeKeyboardAvoidanceMetrics(value: unknown): KeyboardAvoidanceMetrics | null {
  if (!validKeyboardAvoidanceMetrics(value)) return null;
  const rows = value.rows;
  const metrics: KeyboardAvoidanceMetrics = {
    rows,
    cursorRow: Math.min(value.cursorRow, rows - 1),
    altScreen: value.altScreen,
  };
  if (value.rowPitch !== undefined) metrics.rowPitch = value.rowPitch;
  if (value.top !== undefined) metrics.top = value.top;
  return metrics;
}
export function validGrid(rows: unknown, cols: unknown): rows is number {
  return Number.isSafeInteger(rows) && Number.isSafeInteger(cols) && (rows as number) >= 1 && (cols as number) >= 1
    && (rows as number) <= RUNTIME_LIMITS.rows && (cols as number) <= RUNTIME_LIMITS.cols && (rows as number) * (cols as number) <= RUNTIME_LIMITS.cells;
}
