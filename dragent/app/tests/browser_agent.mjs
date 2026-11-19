// The page side of `dragent browser` (app/src/browser/agent.js) in a real
// browser engine. Run with DRAGENT_PLAYWRIGHT_MODULE pointing to an installed
// playwright-core module and DRAGENT_CHROME to a Chrome or Chromium binary:
//   node --test app/tests/browser_agent.mjs
import { test, before, after, beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.DRAGENT_PLAYWRIGHT_MODULE || 'playwright-core');
const agentScript = await readFile(new URL('../src/browser/agent.js', import.meta.url), 'utf8');

const fixture = `<!doctype html><html><head><title>Invite</title></head><body>
<h1>Invite a teammate</h1>
<form id="invite" novalidate>
  <label for="email">Email</label><input id="email" type="email" required>
  <span id="pw-label">Password</span><input type="password" aria-labelledby="pw-label" value="hunter2">
  <label><input type="checkbox" id="terms"> I agree</label>
  <select id="role" aria-label="Role"><option>Viewer</option><option>Editor</option></select>
  <input type="file" aria-label="Avatar">
  <button id="send" type="submit" disabled>Send invite</button>
</form>
<div role="alert" id="alert"></div>
<button id="delete">Delete invite</button>
<button id="rename">Rename</button>
<a href="/help" target="_blank">Help</a>
<ul id="results"></ul>
<button id="load">Load results</button>
<iframe title="details" srcdoc="<button>Inner action</button>"></iframe>
<canvas width="300" height="150"></canvas>
<div id="host"></div>
<div style="height:2000px"></div>
<button id="far">Far away</button>
<script>
  const terms = document.getElementById('terms');
  terms.addEventListener('change', () => { document.getElementById('send').disabled = !terms.checked; });
  document.getElementById('invite').addEventListener('submit', (e) => {
    e.preventDefault();
    const email = document.getElementById('email');
    document.getElementById('alert').textContent = email.checkValidity() ? 'Invite sent' : 'Enter a valid email';
  });
  document.getElementById('delete').onclick = () => {
    document.getElementById('alert').textContent = confirm('Delete this invite?') ? 'Deleted' : 'Kept';
  };
  document.getElementById('rename').onclick = () => {
    const name = prompt('New name?', 'old');
    document.getElementById('alert').textContent = 'Name: ' + name;
  };
  document.getElementById('load').onclick = () => {
    fetch('/slow').then(r => r.text()).then(t => {
      document.getElementById('results').innerHTML = '<li>' + t + '</li>';
    });
    fetch('/missing');
  };
  const shadow = document.getElementById('host').attachShadow({ mode: 'open' });
  shadow.innerHTML = '<button>Shadow button</button>';
  window.clicks = 0;
  document.getElementById('far').onclick = () => window.clicks++;
</script></body></html>`;

let browser, page, server, url;

before(async () => {
  server = createServer((req, res) => {
    if (req.url === '/slow') return setTimeout(() => res.end('Ada Lovelace'), 700);
    if (req.url === '/missing') { res.statusCode = 404; return res.end('nope'); }
    res.setHeader('content-type', 'text/html');
    res.end(fixture);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  url = `http://127.0.0.1:${server.address().port}/`;
  browser = await chromium.launch({ executablePath: process.env.DRAGENT_CHROME, headless: true });
});

after(async () => {
  await browser?.close();
  server?.close();
});

beforeEach(async () => {
  await page?.close();
  page = await browser.newPage({ viewport: { width: 1000, height: 800 } });
  // As every page in Dragent gets it: at the start of the document.
  await page.addInitScript(agentScript);
  await page.goto(url);
});

const call = async (expression) => JSON.parse(await page.evaluate(expression));
const ok = async (expression) => {
  const answer = await call(expression);
  assert.ok(!answer.error, `${expression}: ${answer.error}`);
  return answer.ok;
};
const snapshot = () => ok('window.__dragentAgent.snapshot()');
const refFor = (snap, text) => {
  const line = snap.elements.find((element) => element.includes(text));
  assert.ok(line, `no element with “${text}” in:\n${snap.elements.join('\n')}`);
  return line.match(/^\[(e\d+)\]/)[1];
};

test('a snapshot names elements, their states and what else is on the page', async () => {
  const snap = await snapshot();
  const line = (text) => snap.elements.find((element) => element.includes(text));
  assert.match(line('Email'), /input\[email\] "Email".*\(required\)/);
  assert.match(line('"Password"'), /\(filled\)/);
  assert.ok(!snap.elements.join('\n').includes('hunter2'), 'passwords never leak');
  assert.match(line('I agree'), /\(unchecked\)/);
  assert.match(line('Role'), /options: Viewer \| Editor/);
  assert.match(line('Avatar'), /file picker/);
  assert.match(line('Send invite'), /\(disabled\)/);
  assert.match(line('Help'), /opens a new tab/);
  assert.match(line('Inner action'), /in frame details/);
  assert.ok(line('Shadow button'), 'open shadow roots are searched');
  assert.equal(snap.drawn.length, 1);
  assert.deepEqual(snap.headings, ['h1: Invite a teammate']);
});

test('references never repeat, and an old one fails as stale', async () => {
  const first = await snapshot();
  const old = refFor(first, 'Load results');
  const second = await snapshot();
  const fresh = refFor(second, 'Load results');
  assert.notEqual(old, fresh);
  const stale = await call(`window.__dragentAgent.click(${JSON.stringify(old)})`);
  assert.equal(stale.code, 'stale_ref');
  assert.equal(await page.evaluate(() => window.clicks), 0);
});

test('a disabled button refuses, a validation error is read back, and a valid form submits', async () => {
  let snap = await snapshot();
  const disabled = await call(`window.__dragentAgent.click(${JSON.stringify(refFor(snap, 'Send invite'))})`);
  assert.equal(disabled.code, 'element_disabled');
  const typed = await ok(`window.__dragentAgent.type(${JSON.stringify(refFor(snap, 'Email'))}, 'not-an-email', false)`);
  assert.match(typed, /The field reports: /);
  await ok(`window.__dragentAgent.click(${JSON.stringify(refFor(snap, 'I agree'))})`);
  snap = await snapshot();
  assert.match(snap.elements.find((element) => element.includes('Email')), /\(invalid/);
  await ok(`window.__dragentAgent.click(${JSON.stringify(refFor(snap, 'Send invite'))})`);
  snap = await snapshot();
  assert.deepEqual(snap.alerts, ['Enter a valid email']);
  await ok(`window.__dragentAgent.type(${JSON.stringify(refFor(snap, 'Email'))}, 'ada@example.com', true)`);
  snap = await snapshot();
  assert.deepEqual(snap.alerts, ['Invite sent']);
  const file = await call(`window.__dragentAgent.type(${JSON.stringify(refFor(snap, 'Avatar'))}, '/etc/passwd', false)`);
  assert.equal(file.code, 'unsupported');
  const option = await call(`window.__dragentAgent.type(${JSON.stringify(refFor(snap, 'Role'))}, 'Owner', false)`);
  assert.equal(option.code, 'no_such_option');
  assert.match(option.error, /“Viewer”, “Editor”/);
});

test('dialogs never block: dismissed unless an answer was set, and on the record', async () => {
  let snap = await snapshot();
  const dismissed = await ok(`window.__dragentAgent.click(${JSON.stringify(refFor(snap, 'Delete invite'))})`);
  assert.match(dismissed, /A confirm dialog appeared: “Delete this invite\?” \(dismissed\)/);
  assert.equal(await page.textContent('#alert'), 'Kept');
  await ok('window.__dragentAgent.dialog(true, null)');
  await ok(`window.__dragentAgent.click(${JSON.stringify(refFor(snap, 'Delete invite'))})`);
  assert.equal(await page.textContent('#alert'), 'Deleted');
  await ok('window.__dragentAgent.dialog(true, "Grace")');
  await ok(`window.__dragentAgent.click(${JSON.stringify(refFor(snap, 'Rename'))})`);
  assert.equal(await page.textContent('#alert'), 'Name: Grace');
  const log = await ok('window.__dragentAgent.readConsole(false, null)');
  assert.match(log, /\[dialog\] confirm “Delete this invite\?” → dismissed/);
  assert.match(log, /\[dialog\] prompt “New name\?” → accepted with “Grace”/);
});

test('waiting: conditions are checked as they are, and failed requests are logged', async () => {
  const snap = await snapshot();
  const check = (conditions) => ok(`window.__dragentAgent.check(${JSON.stringify(conditions)})`);
  assert.deepEqual(await check({ text: 'Ada Lovelace' }), { met: false, detail: 'text “Ada Lovelace” isn\'t on the page' });
  await ok(`window.__dragentAgent.click(${JSON.stringify(refFor(snap, 'Load results'))})`);
  const busy = await check({ idle: true });
  assert.equal(busy.met, false);
  assert.match(busy.detail, /request|changing/);
  const deadline = Date.now() + 5000;
  let state;
  do {
    state = await check({ text: 'Ada Lovelace', selector: '#results li', idle: true });
    if (!state.met) await new Promise((resolve) => setTimeout(resolve, 100));
  } while (!state.met && Date.now() < deadline);
  assert.equal(state.met, true, state.detail);
  const errors = await ok('window.__dragentAgent.readConsole(false, ["error", "network"])');
  assert.match(errors, /\[network\] GET .*\/missing → 404/);
  const badSelector = await call('window.__dragentAgent.check({ selector: "##" })');
  assert.equal(badSelector.code, 'invalid_input');
});

test('Tab moves focus, scrolling reaches an element, and promises can be awaited', async () => {
  const snap = await snapshot();
  await page.focus('#email');
  const pressed = await ok('window.__dragentAgent.press("Tab")');
  assert.match(pressed, /Focus is on .*"Password"/);
  await ok(`window.__dragentAgent.scroll(${JSON.stringify(refFor(snap, 'Far away'))})`);
  assert.ok(await page.evaluate(() => scrollY > 500));
  const started = await ok('window.__dragentAgent.evaluate("fetch(\'/slow\').then(r => r.text())", true)');
  assert.ok(Number.isInteger(started.pending));
  let settled;
  const deadline = Date.now() + 5000;
  do {
    settled = await ok(`window.__dragentAgent.settled(${started.pending})`);
    if (!settled.met) await new Promise((resolve) => setTimeout(resolve, 100));
  } while (!settled.met && Date.now() < deadline);
  assert.equal(settled.detail, 'Ada Lovelace');
  assert.equal(await ok('window.__dragentAgent.evaluate("1 + 1", false)'), '2');
  const thrown = await call('window.__dragentAgent.evaluate("nope.nope", false)');
  assert.equal(thrown.code, 'script_error');
});
