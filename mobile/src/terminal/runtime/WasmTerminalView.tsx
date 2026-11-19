import { useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type ComponentRef } from 'react';
import { Linking, StyleSheet, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { WebView } from 'react-native-webview';
import { AppText, EmptyState } from '../../components';
import { useTheme } from '../../theme';
import { createRequestId } from '../../transport/socket';
import { RuntimeBridge } from './RuntimeBridge';
import { terminalDocument } from './document';
import type { WasmTerminalViewProps } from './types';

export type { WasmTerminalViewProps } from './types';

/** Owns only a renderer. The native client remains the only network/auth/input authority. */
export function WasmTerminalView(props: WasmTerminalViewProps) {
  const [generation, setGeneration] = useState(0);
  const owner = useMemo(() => createRequestId(), [props.store]);
  return <NativeCanvas key={`${owner}:${generation}`} {...props} onRestart={() => setGeneration((value) => value + 1)} />;
}
function NativeCanvas({ onRestart, ...props }: WasmTerminalViewProps & { onRestart: () => void }) {
  const { theme } = useTheme();
  const web = useRef<ComponentRef<typeof WebView>>(null);
  const latest = useRef({ ...props, palette: theme.terminal });
  latest.current = { ...props, palette: theme.terminal };
  const [error, setError] = useState<string | null>(null);
  const [id] = useState(() => createRequestId().replace(/[^a-zA-Z0-9_-]/gu, '_'));
  const html = useMemo(() => terminalDocument(id), [id]);
  const bridge = useRef<RuntimeBridge | null>(null);
  // "Copied" for a moment after Copy, over the terminal.
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (copiedTimer.current) clearTimeout(copiedTimer.current); }, []);
  const copy = useRef((text: string) => {
    void Clipboard.setStringAsync(text).then(() => {
      setCopied(true);
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
      copiedTimer.current = setTimeout(() => setCopied(false), 1400);
    }).catch(() => {});
  }).current;
  useLayoutEffect(() => {
    const owned = new RuntimeBridge(id, props.store,
      () => ({ connected: latest.current.connected, canWrite: latest.current.canWrite, canResize: latest.current.canResize, holdFit: latest.current.holdFit, palette: latest.current.palette }), {
        send: (packet) => { if (!web.current) throw new Error('Renderer unavailable'); web.current.postMessage(JSON.stringify(packet)); },
        onInput: (text) => latest.current.onInput(text),
        onResize: (rows, cols) => latest.current.onResize(rows, cols),
        onMetrics: (metrics) => latest.current.onMetrics?.(metrics),
        onError: setError,
        onCopy: copy,
        onLink: (url) => { void Linking.openURL(url).catch(() => {}); },
      });
    bridge.current = owned;
    const detach = props.store.subscribe(owned.invalidate);
    owned.start();
    return () => { bridge.current = null; detach(); owned.dispose(); };
  }, [id, props.store]);
  useImperativeHandle(props.inputControl, () => ({ blurInput: () => bridge.current?.blurInput() }), []);
  useEffect(() => bridge.current?.invalidate(), [props.connected, props.canWrite, props.canResize, props.holdFit, theme.terminal]);
  if (error) return <View style={[styles.fill, { backgroundColor: theme.terminal.background }]}><EmptyState title="Terminal renderer unavailable" message={error} actionLabel="Restart renderer" onAction={onRestart} /></View>;
  return <View style={styles.fill}><WebView
    ref={web}
    style={[styles.fill, { backgroundColor: theme.terminal.background }]}
    source={{ html, baseUrl: 'https://terminal.dragent.invalid/' }}
    originWhitelist={['https://terminal.dragent.invalid', 'about:blank']}
    onShouldStartLoadWithRequest={(request) => request.url === 'about:blank' || request.url === 'https://terminal.dragent.invalid/'}
    onMessage={(event) => bridge.current?.receive(event.nativeEvent.data)}
    onError={() => setError('The local terminal renderer could not load.')}
    onContentProcessDidTerminate={() => setError('The local terminal renderer was stopped by the device.')}
    javaScriptEnabled
    domStorageEnabled={false}
    allowFileAccess={false}
    allowFileAccessFromFileURLs={false}
    allowUniversalAccessFromFileURLs={false}
    mixedContentMode="never"
    javaScriptCanOpenWindowsAutomatically={false}
    setSupportMultipleWindows={false}
    thirdPartyCookiesEnabled={false}
    sharedCookiesEnabled={false}
    scrollEnabled={false}
    bounces={false}
    overScrollMode="never"
    textZoom={100}
    automaticallyAdjustContentInsets={false}
    keyboardDisplayRequiresUserAction
    accessibilityLabel="Live terminal surface"
  />{copied ? <View pointerEvents="none" style={styles.toastHost}><View style={[styles.toast, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}><AppText variant="caption" accessibilityLiveRegion="polite">Copied</AppText></View></View> : null}</View>;
}
const styles = StyleSheet.create({
  fill: { flex: 1, minHeight: 0 },
  toastHost: { position: 'absolute', left: 0, right: 0, top: 12, alignItems: 'center' },
  toast: { paddingHorizontal: 14, paddingVertical: 7, borderRadius: 16, borderWidth: StyleSheet.hairlineWidth },
});
