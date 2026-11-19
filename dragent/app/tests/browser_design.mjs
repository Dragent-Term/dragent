// Run with DRAGENT_PLAYWRIGHT_MODULE pointing to an installed playwright-core module.
import { test, before, after, beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.DRAGENT_PLAYWRIGHT_MODULE || 'playwright-core');
const script = await readFile(new URL('../src/browser/design.js', import.meta.url), 'utf8');
let browser, page, server, url, notes, messages, config;
const fixture = `<!doctype html><html><head><title>Design fixture</title><style>body{margin:0}</style></head>
<body style="margin:0;height:1800px;background:#f6f6f6;font:16px sans-serif">
<main style="position:absolute;top:150px;left:100px;width:400px;padding:35px;background:white" data-source="src/Card.tsx:12">
<h1>Account settings</h1><button id="save" style="background:rgb(20,100,200);padding:12px 24px;border:0;color:white">Save changes</button>
<input type="password" value="must-not-leak"><p>Keep the same spacing as this card.</p>
</main><script>window.activations=0;document.querySelector('#save').onclick=()=>window.activations++;</script></body></html>`;
const palette = { bg: 'rgba(40,42,54,1)', raised: 'rgba(52,55,70,1)', fg: 'rgba(248,248,242,0.9)', muted: 'rgba(248,248,242,0.6)',
  line: 'rgba(80,83,100,1)', accent: 'rgba(189,147,249,1)', accent_fg: 'rgba(15,18,22,1)', danger: 'rgba(255,85,85,1)', warning: 'rgba(241,250,140,1)' };
const sync = async () => page.evaluate(config => window.__dragentDesign.configure(config), { ...config, notes });
const ui = selector => page.locator('[data-dragent-design]').locator(selector);
const pointAt = async (selector, click = false) => {
  const box = await page.locator(selector).boundingBox();
  const x = box.x + box.width / 2, y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  if (click) await page.mouse.click(x, y);
};
const wait = async condition => {
  const end = Date.now() + 5000;
  while (!condition()) { if (Date.now() > end) throw new Error('Timed out'); await new Promise(r => setTimeout(r, 20)); }
};

async function openPage({ openShadow = true } = {}) {
  await page?.close();
  page = await browser.newPage({ viewport: { width: 1100, height: 850 }, deviceScaleFactor: 2 });
  // Open the shadow root so Playwright can reach the controls; one test keeps it closed, as shipped.
  if (openShadow) {
    await page.addInitScript(() => {
      const attach = Element.prototype.attachShadow;
      Element.prototype.attachShadow = function(options) { return attach.call(this, { ...options, mode: 'open' }); };
    });
  }
  notes = []; messages = [];
  config = { token: randomUUID(), enabled: true, dark: true, sending: false, target: 'Claude Code', palette };
  await page.exposeFunction('hostMessage', async raw => {
    const message = JSON.parse(raw.slice('dragent-design:'.length)); messages.push(message);
    if (message.kind === 'pick' || message.kind === 'markup') {
      notes.push({ id: randomUUID(), selection: message.selection, comment: '', intent: 'change',
        screenshot: true, capture_error: null, markup: message.kind === 'markup', preview: '' });
      await sync();
    } else if (message.kind === 'update') {
      Object.assign(notes.find(n => n.id === message.id), { comment: message.comment, intent: message.intent });
    } else if (message.kind === 'remove') { notes = notes.filter(n => n.id !== message.id); await sync(); }
    else if (message.kind === 'clear') { notes = []; await sync(); }
    else if (message.kind === 'close') { config.enabled = false; await sync(); }
    else if (message.kind === 'capture_markup') {
      const image = 'data:image/png;base64,' + (await page.screenshot()).toString('base64');
      await page.evaluate(({ image, selection }) => window.__dragentDesign.captured(image, selection), { image, selection: message.selection });
    }
  });
  await page.goto(url);
  await page.evaluate(() => { window.ipc = { postMessage: message => window.hostMessage(message) }; });
  await page.evaluate(script);
  await sync();
}

before(async () => {
  server = createServer((req, res) => { res.writeHead(200, { 'Content-Type': 'text/html' }); res.end(fixture); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  url = 'http://127.0.0.1:' + server.address().port;
  browser = await chromium.launch({ headless: true,
    executablePath: process.env.DRAGENT_CHROME || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    args: ['--disable-background-networking', '--disable-sync', '--no-first-run'] });
});
after(async () => { await browser?.close(); await new Promise(resolve => server?.close(resolve)); });
beforeEach(() => openPage());

test('a real pointer picks through the closed overlay, as shipped', async () => {
  await openPage({ openShadow: false });
  await pointAt('#save', true);
  await wait(() => notes.length === 1);
  assert.equal(notes[0].selection.selector, '#save');
  assert.equal(await page.evaluate(() => window.activations), 0);
  // Enter in the card saves; Escape then leaves design mode.
  await page.keyboard.type('Match the card padding');
  await wait(() => messages.some(m => m.kind === 'update' && m.comment === 'Match the card padding'));
  await page.keyboard.press('Enter');
  await page.keyboard.press('Escape');
  await wait(() => messages.some(m => m.kind === 'close'));
});

test('a click opens a note card at the element and picking resumes after saving', async () => {
  await pointAt('#save');
  await ui('.outline').waitFor();
  await pointAt('#save', true);
  await wait(() => notes.length === 1);
  const s = notes[0].selection;
  assert.equal(s.selector, '#save'); assert.equal(s.text, 'Save changes'); assert.equal(s.role, 'button');
  assert.equal(s.source, 'src/Card.tsx:12'); assert.equal(s.styles['background-color'], 'rgb(20, 100, 200)');
  assert.match(s.path, /main > button#save$/);
  assert.ok(s.rect.width > 30); assert.equal(s.viewport.width, 1100);
  assert.equal(await page.evaluate(() => window.activations), 0);
  const card = ui('.card'); await card.waitFor();
  const [cardBox, buttonBox] = [await card.boundingBox(), await page.locator('#save').boundingBox()];
  assert.ok(cardBox.y >= buttonBox.y + buttonBox.height, 'The card sits below the element');
  assert.equal(await ui('textarea').evaluate(node => node.getRootNode().activeElement === node), true);
  await page.keyboard.type('Match the card padding');
  await ui('select[aria-label="What kind of request"]').selectOption('fix');
  await wait(() => notes[0].intent === 'fix' && notes[0].comment === 'Match the card padding');
  await ui('textarea').press('Enter');
  await card.waitFor({ state: 'detached' });
  await ui('.pin').waitFor();
  await pointAt('h1');
  await ui('.outline').waitFor();
  assert.ok(!messages.some(m => m.kind === 'send'), 'Page controls must not send to an agent');
});

test('saving needs a request; Escape discards an empty new note, then exits', async () => {
  await pointAt('#save', true); await wait(() => notes.length === 1);
  await ui('button[aria-label="Save this note · Enter"]').click();
  await ui('.error').waitFor();
  await page.keyboard.press('Escape');
  await wait(() => notes.length === 0);
  await ui('.card').waitFor({ state: 'detached' });
  await page.keyboard.press('Escape');
  await page.locator('[data-dragent-design]').waitFor({ state: 'detached' });
  await page.locator('#save').click();
  assert.equal(await page.evaluate(() => window.activations), 1);
});

test('parent picking redacts form values', async () => {
  await pointAt('#save'); await ui('.outline').waitFor(); await page.keyboard.press('Alt+ArrowUp');
  await pointAt('#save', true); await wait(() => notes.length === 1);
  assert.equal(notes[0].selection.tag, 'main');
  assert.ok(!notes[0].selection.html.includes('must-not-leak'));
});

test('C copies the hovered element without making a note', async () => {
  await pointAt('#save'); await ui('.outline').waitFor();
  await page.keyboard.press('c');
  await wait(() => messages.some(m => m.kind === 'copy_element'));
  assert.equal(messages.find(m => m.kind === 'copy_element').selection.selector, '#save');
  assert.equal(notes.length, 0);
});

test('the overlay takes its colours from the app theme', async () => {
  await pointAt('#save', true); await ui('.card').waitFor();
  const style = await ui('.card').evaluate(node => {
    const s = getComputedStyle(node);
    return { color: s.color, background: s.backgroundColor, font: s.fontFamily, scheme: getComputedStyle(node.parentElement.parentElement).colorScheme };
  });
  assert.equal(style.color, 'rgba(248, 248, 242, 0.9)');
  assert.equal(style.background, 'rgb(40, 42, 54)');
  assert.match(style.font, /system-ui/);
  assert.equal(style.scheme, 'dark');
  const primary = await ui('.btn.primary').evaluate(node => getComputedStyle(node).color);
  assert.equal(primary, 'rgb(15, 18, 22)');
});

test('native send flush receives the latest edit before its acknowledgement', async () => {
  await pointAt('#save', true); await wait(() => notes.length === 1);
  await ui('textarea').fill('The very latest change');
  const request = randomUUID();
  await page.evaluate(request => window.__dragentDesign.flush(request), request);
  await wait(() => messages.some(m => m.kind === 'flushed' && m.request === request));
  const update = messages.findIndex(m => m.kind === 'update' && m.comment === 'The very latest change');
  const ack = messages.findIndex(m => m.kind === 'flushed' && m.request === request);
  assert.ok(update >= 0 && update < ack);
  assert.equal(notes[0].comment, 'The very latest change');
});

test('an agent-list refresh preserves the pending comment and its focus', async () => {
  await pointAt('#save', true); await wait(() => notes.length === 1);
  await ui('textarea').fill('A comment still being typed');
  await sync();
  assert.equal(await ui('textarea').inputValue(), 'A comment still being typed');
  assert.equal(await ui('textarea').evaluate(node => node.getRootNode().activeElement === node), true);
  await wait(() => notes[0].comment === 'A comment still being typed');
});

test('iframe picking selects the frame without activating its document', async () => {
  await page.evaluate(() => {
    const frame = document.createElement('iframe'); frame.id = 'preview-frame';
    frame.style.cssText = 'position:absolute;left:550px;top:180px;width:250px;height:180px';
    frame.srcdoc = '<button style="width:240px;height:160px" onclick="parent.frameActivated=true">Inside frame</button>';
    document.body.append(frame);
  });
  await pointAt('#preview-frame', true); await wait(() => notes.length === 1);
  assert.equal(notes[0].selection.tag, 'iframe');
  assert.equal(await page.evaluate(() => Boolean(window.frameActivated)), false);
});

test('notes survive a reload and can be reopened from the list and deleted', async () => {
  await pointAt('#save', true); await wait(() => notes.length === 1);
  await ui('textarea').fill('Keep this feedback through reload');
  await wait(() => notes[0].comment.includes('reload'));
  await page.reload(); await page.evaluate(() => { window.ipc = { postMessage: message => window.hostMessage(message) }; });
  await page.evaluate(script); await sync();
  await ui('button[aria-label="Review your notes"]').click();
  await ui('.tray').waitFor();
  await ui('button[aria-label="Open note 1"]').click();
  assert.equal(await ui('textarea').inputValue(), 'Keep this feedback through reload');
  await ui('button[aria-label="Delete this note"]').click();
  await wait(() => notes.length === 0);
  await ui('.card').waitFor({ state: 'detached' });
});

test('markup captures the page, keeps undo and redo, and opens the new note', async () => {
  await page.evaluate(() => window.__dragentDesign.tool('draw'));
  const canvas = ui('canvas'); await canvas.waitFor();
  const box = await canvas.boundingBox();
  await page.mouse.move(box.x + 160, box.y + 180); await page.mouse.down();
  await page.mouse.move(box.x + 330, box.y + 280); await page.mouse.up();
  await ui('button[aria-label^="Undo"]').click();
  await ui('button[aria-label="Redo"]').click();
  await ui('button[aria-label="Attach this marked-up screenshot as a note"]').click();
  await wait(() => notes.length === 1);
  const message = messages.find(m => m.kind === 'markup');
  assert.ok(message.image.startsWith('data:image/png;base64,iVBOR'));
  assert.ok(message.image.length < 11 * 1024 * 1024); assert.equal(notes[0].markup, true);
  await ui('.card textarea[placeholder="What should change in this screenshot?"]').waitFor();
});
