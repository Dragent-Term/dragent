import { useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { StyleSheet, View } from 'react-native';
import { EmptyState } from '../../components';
import { useTheme } from '../../theme';
import { createRequestId } from '../../transport/socket';
import { RuntimeBridge } from './RuntimeBridge';
import { terminalDocument } from './document';
import type { WasmTerminalViewProps } from './types';

export type { WasmTerminalViewProps } from './types';

export function WasmTerminalView(props: WasmTerminalViewProps) {
  const [generation, setGeneration] = useState(0);
  const owner = useMemo(() => createRequestId(), [props.store]);
  return <WebCanvas key={`${owner}:${generation}`} {...props} onRestart={() => setGeneration((value) => value + 1)} />;
}
function WebCanvas({ onRestart, ...props }: WasmTerminalViewProps & { onRestart: () => void }) {
  const { theme } = useTheme();
  const frame = useRef<HTMLIFrameElement>(null);
  const latest = useRef({ ...props, palette: theme.terminal });
  latest.current = { ...props, palette: theme.terminal };
  const [error, setError] = useState<string | null>(null);
  const [id] = useState(() => createRequestId().replace(/[^a-zA-Z0-9_-]/gu, '_'));
  const html = useMemo(() => terminalDocument(id), [id]);
  const bridge = useRef<RuntimeBridge | null>(null);
  useLayoutEffect(() => {
    const owned = new RuntimeBridge(id, props.store,
      () => ({ connected: latest.current.connected, canWrite: latest.current.canWrite, canResize: latest.current.canResize, holdFit: latest.current.holdFit, palette: latest.current.palette }), {
        send: (packet) => { if (!frame.current?.contentWindow) throw new Error('Renderer unavailable'); frame.current.contentWindow.postMessage(JSON.stringify(packet), '*'); },
        onInput: (text) => latest.current.onInput(text), onResize: (rows, cols) => latest.current.onResize(rows, cols), onMetrics: (metrics) => latest.current.onMetrics?.(metrics), onError: setError,
      });
    bridge.current = owned;
    const message = (event: MessageEvent) => { if (event.source === frame.current?.contentWindow) owned.receive(event.data); };
    window.addEventListener('message', message);
    const detach = props.store.subscribe(owned.invalidate);
    owned.start();
    return () => { bridge.current = null; window.removeEventListener('message', message); detach(); owned.dispose(); };
  }, [id, props.store]);
  useImperativeHandle(props.inputControl, () => ({ blurInput: () => bridge.current?.blurInput() }), []);
  useEffect(() => bridge.current?.invalidate(), [props.connected, props.canWrite, props.canResize, props.holdFit, theme.terminal]);
  if (error) return <View style={[styles.fill, { backgroundColor: theme.terminal.background }]}><EmptyState title="Terminal renderer unavailable" message={error} actionLabel="Restart renderer" onAction={onRestart} /></View>;
  return <iframe ref={frame} title="Live terminal surface" sandbox="allow-scripts" srcDoc={html} style={{ display: 'block', flex: 1, width: '100%', minHeight: 0, border: 0, background: theme.terminal.background }} />;
}
const styles = StyleSheet.create({ fill: { flex: 1, minHeight: 0 } });
