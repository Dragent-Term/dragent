import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const source = new URL('../../auth-portal/public/wasm/', import.meta.url);
const target = new URL('../src/terminal/runtime/wasmAsset.generated.ts', import.meta.url);
const glue = await readFile(new URL('dragent_terminal_wasm.js', source), 'utf8');
const wasm = await readFile(new URL('dragent_terminal_wasm_bg.wasm', source));
if (!glue.includes('export class Terminal') || !glue.includes('export { initSync, __wbg_init as default };') || /<\/script/iu.test(glue)) throw new Error('Unexpected terminal WASM glue; review the canonical artifact before packaging.');
// The document runs one function: the runtime, with the pure render helpers
// and the WebGL2 renderer defined inside it and handed over as arguments.
const runtime = (await readFile(new URL('../src/terminal/runtime/browserRuntime.js', import.meta.url), 'utf8')).replace('export function browserRuntime', 'function runtime').trim();
const part = async (file, name) => (await readFile(new URL(`../src/terminal/runtime/${file}`, import.meta.url), 'utf8')).replace(`export function ${name}`, `function ${name}`).trim();
const kit = await part('renderKit.js', 'renderKit');
const gpuKit = await part('gpuKit.js', 'gpuKit');
const gpu = await part('gpuRenderer.js', 'gpuRenderer');
if ([runtime, kit, gpuKit, gpu].some((source) => /^\s*(export|import)\b/mu.test(source))) throw new Error('Runtime sources must be self-contained functions without imports or other exports.');
const browser = `function browserRuntime(Terminal, id, limits, options) {\n${kit}\n${gpuKit}\n${gpu}\nreturn (${runtime})(Terminal, id, limits, renderKit(), options, gpuKit(), gpuRenderer);\n}`;
if (/<\/script/iu.test(browser)) throw new Error('Runtime source may not contain a closing script tag.');
const hash = createHash('sha256').update(glue).update(wasm).update(browser).digest('hex');
const output = '// Generated from the canonical Dragent terminal WASM assets. Do not edit.\n'
 + `export const TERMINAL_BROWSER_SOURCE = ${JSON.stringify(browser)};\n`
 + `export const TERMINAL_WASM_HASH = ${JSON.stringify(hash)};\n`
 + `export const TERMINAL_WASM_GLUE = ${JSON.stringify(glue)};\n`
 + `export const TERMINAL_WASM_BASE64 = ${JSON.stringify(wasm.toString('base64'))};\n`;
if (process.argv.includes('--check')) {
 if (await readFile(target, 'utf8').catch(() => '') !== output) throw new Error('Mobile terminal WASM assets are stale. Run node scripts/sync-terminal-wasm.mjs.');
 console.log(`Terminal WASM assets in sync (${wasm.length} bytes, ${hash.slice(0,12)}).`);
} else {
 await writeFile(target, output);
 console.log(`Packaged terminal WASM: ${fileURLToPath(target)} (${wasm.length} bytes).`);
}
