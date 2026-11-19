// Agent control of a browser pane, on the page side.
//
// Installed at the start of every page (and again by any command that finds
// it missing), it records what the page does that an agent needs to know
// (console messages, failed requests, dialogs, activity) and answers the
// `dragent browser` commands: a snapshot that numbers the elements an agent
// can act on, actions on those elements, and conditions to wait for.
// Every answer is a JSON string: {"ok": value} or {"error": text, "code": code}.
(() => {
  if (window.__dragentAgent) return;

  const clean = (text) => String(text == null ? '' : text).replace(/\s+/g, ' ').trim();
  const done = (ok) => JSON.stringify({ ok });
  const fail = (code, error) => JSON.stringify({ error, code });

  // ── What the page did ────────────────────────────────────────────────────
  const log = [];
  const push = (level, text) => {
    log.push({ level, text: String(text).slice(0, 2000) });
    if (log.length > 300) log.shift();
  };
  const show = (value) => {
    if (typeof value === 'string') return value;
    if (value instanceof Error) return value.stack || String(value);
    try { return JSON.stringify(value); } catch (_) { return String(value); }
  };
  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    const original = console[level];
    console[level] = function (...args) { push(level, args.map(show).join(' ')); return original.apply(this, args); };
  }
  addEventListener('error', (e) => {
    if (e.target && e.target !== window && e.target.tagName) {
      const src = e.target.src || e.target.href || '';
      push('network', `${e.target.tagName.toLowerCase()} failed to load: ${src}`);
    } else {
      push('error', e.error && e.error.stack ? e.error.stack : `${e.message} (${e.filename}:${e.lineno})`);
    }
  }, true);
  addEventListener('unhandledrejection', (e) => push('error', `Unhandled promise rejection: ${show(e.reason)}`));

  // Requests in flight and failed ones: what "the page is quiet" means.
  let inflight = 0;
  let lastActivity = Date.now();
  const touch = () => { lastActivity = Date.now(); };
  if (typeof window.fetch === 'function') {
    const fetch = window.fetch;
    window.fetch = function (input, init) {
      const method = (init && init.method) || (input && input.method) || 'GET';
      const url = String((input && input.url) || input);
      inflight++; touch();
      return fetch.apply(this, arguments).then((response) => {
        if (response.status >= 400) push('network', `${method.toUpperCase()} ${url} → ${response.status} ${response.statusText}`);
        return response;
      }, (error) => {
        push('network', `${method.toUpperCase()} ${url} failed: ${show(error)}`);
        throw error;
      }).finally(() => { inflight--; touch(); });
    };
  }
  if (window.XMLHttpRequest) {
    const open = XMLHttpRequest.prototype.open;
    const send = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function (method, url) {
      this.__dragent = { method: String(method || 'GET').toUpperCase(), url: String(url) };
      return open.apply(this, arguments);
    };
    XMLHttpRequest.prototype.send = function () {
      const request = this.__dragent || { method: 'GET', url: '' };
      inflight++; touch();
      this.addEventListener('loadend', () => {
        inflight--; touch();
        if (this.status === 0) push('network', `${request.method} ${request.url} failed`);
        else if (this.status >= 400) push('network', `${request.method} ${request.url} → ${this.status} ${this.statusText}`);
      });
      return send.apply(this, arguments);
    };
  }
  try {
    new MutationObserver(touch).observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  } catch (_) {}

  // Dialogs never block the page: the next one gets the answer the agent
  // chose (once), otherwise it's dismissed; each is on the record.
  const dialogs = [];
  let nextAnswer = null;
  const answerDialog = (kind, message, fallback) => {
    const answer = nextAnswer || { accept: false };
    nextAnswer = null;
    let value;
    if (kind === 'alert') value = undefined;
    else if (kind === 'confirm') value = !!answer.accept;
    else value = answer.accept ? (answer.text != null ? String(answer.text) : String(fallback == null ? '' : fallback)) : null;
    const outcome = kind === 'alert' ? 'shown' : answer.accept ? 'accepted' : 'dismissed';
    const entry = { kind, message: clean(message).slice(0, 500), outcome };
    dialogs.push(entry);
    if (dialogs.length > 50) dialogs.shift();
    push('dialog', `${kind} “${entry.message}” → ${outcome}${kind === 'prompt' && answer.accept ? ` with “${value}”` : ''}`);
    touch();
    return value;
  };
  window.alert = (message) => { answerDialog('alert', message); };
  window.confirm = (message) => answerDialog('confirm', message);
  window.prompt = (message, fallback) => answerDialog('prompt', message, fallback);

  // ── Elements ─────────────────────────────────────────────────────────────
  const INTERACTIVE = 'a[href],button,input:not([type=hidden]),textarea,select,summary,[role=button],[role=link],[role=checkbox],[role=radio],[role=switch],[role=tab],[role=menuitem],[role=menuitemcheckbox],[role=menuitemradio],[role=option],[role=combobox],[role=textbox],[role=searchbox],[role=slider],[role=spinbutton],[contenteditable=""],[contenteditable=true],[tabindex]:not([tabindex="-1"])';
  const LIMIT = 300;

  // References keep counting for the whole tab, so an old one never
  // lands on a different element: it fails as stale instead.
  const nextRef = () => {
    let n = 0;
    try { n = Number(sessionStorage.getItem('__dragentRef') || 0) + 1; sessionStorage.setItem('__dragentRef', String(n)); }
    catch (_) { n = (window.__dragentRefFallback = (window.__dragentRefFallback || 0) + 1); }
    return 'e' + n;
  };

  // The documents to look in: this one, open shadow roots, and frames on
  // this origin. Frames on other origins are listed, not entered.
  const roots = () => {
    const found = [];
    const visit = (root, frame) => {
      found.push({ root, frame });
      for (const el of root.querySelectorAll('*')) {
        if (el.shadowRoot) visit(el.shadowRoot, frame);
        if (el.tagName === 'IFRAME' || el.tagName === 'FRAME') {
          let inner = null;
          try { inner = el.contentDocument; } catch (_) {}
          if (inner && inner.documentElement) visit(inner, (frame ? frame + '/' : '') + (el.getAttribute('title') || el.getAttribute('name') || el.id || 'frame'));
        }
      }
    };
    visit(document, '');
    return found;
  };
  const find = (ref) => {
    for (const { root } of roots()) {
      const el = root.querySelector(`[data-dragent-ref="${ref}"]`);
      if (el) return el;
    }
    return null;
  };
  const stale = (ref) => fail('stale_ref', `${ref} isn't on the page any more (it re-rendered or navigated). Take a new snapshot and use its references.`);

  const shown = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return false;
    const s = el.ownerDocument.defaultView.getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && Number(s.opacity) > 0;
  };
  const disabled = (el) => !!el.disabled || el.getAttribute('aria-disabled') === 'true' || !!el.closest('fieldset[disabled]');
  const nameOf = (el) => {
    const doc = el.ownerDocument;
    const labelledBy = (el.getAttribute('aria-labelledby') || '').split(/\s+/).filter(Boolean)
      .map((id) => doc.getElementById(id)).filter(Boolean).map((node) => node.innerText || node.textContent).join(' ');
    const label = el.labels && el.labels.length ? [...el.labels].map((l) => l.innerText).join(' ') : '';
    const tag = el.tagName.toLowerCase();
    const own = tag === 'select' ? '' : (el.innerText || el.textContent);
    return clean(el.getAttribute('aria-label') || labelledBy || label || own || el.getAttribute('placeholder')
      || el.getAttribute('title') || el.getAttribute('alt') || el.getAttribute('name') || (tag === 'input' && el.type !== 'password' ? el.value : '')).slice(0, 80);
  };
  const roleOf = (el) => {
    const role = el.getAttribute('role');
    if (role) return role;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'input') return `input[${el.type || 'text'}]`;
    if (tag === 'select') return el.multiple ? 'listbox' : 'combobox';
    if (el.isContentEditable && tag !== 'textarea') return 'editable';
    return tag;
  };
  const describe = (el, ref, frame) => {
    const tag = el.tagName.toLowerCase();
    let line = `[${ref}] ${roleOf(el)} "${nameOf(el)}"`;
    if (tag === 'select') line += ` options: ${[...el.options].slice(0, 12).map((o) => clean(o.text)).join(' | ')}${el.options.length > 12 ? ' | …' : ''}`;
    const field = tag === 'input' || tag === 'textarea' || tag === 'select';
    if (field && el.type === 'password') { if (el.value) line += ' (filled)'; }
    else if (field && el.type !== 'checkbox' && el.type !== 'radio' && el.value) line += ` value="${clean(el.value).slice(0, 60)}"`;
    if (el.type === 'checkbox' || el.type === 'radio') line += el.checked ? ' (checked)' : ' (unchecked)';
    for (const state of ['checked', 'pressed', 'selected', 'expanded']) {
      const value = el.getAttribute('aria-' + state);
      if (value === 'true' || value === 'false') line += ` (${state}: ${value})`;
    }
    if (el.type === 'file') line += ' (file picker: can’t be filled from here)';
    if (disabled(el)) line += ' (disabled)';
    if (el.required || el.getAttribute('aria-required') === 'true') line += ' (required)';
    if ((el.validity && !el.validity.valid && el.value !== undefined && (el.value || el.dataset.dragentTouched)) || el.getAttribute('aria-invalid') === 'true') {
      line += ` (invalid${el.validationMessage ? `: ${clean(el.validationMessage).slice(0, 120)}` : ''})`;
    }
    if (tag === 'a') {
      const href = el.getAttribute('href');
      if (href && !href.startsWith('javascript:')) line += ` -> ${href.slice(0, 100)}`;
      if (el.target === '_blank') line += ' (opens a new tab)';
    }
    if (el.ownerDocument.activeElement === el) line += ' (focused)';
    if (el.closest('dialog[open],[role=dialog],[role=alertdialog],[aria-modal=true]')) line += ' (in dialog)';
    if (frame) line += ` (in frame ${frame})`;
    return line;
  };

  const snapshot = () => {
    const all = roots();
    for (const { root } of all) root.querySelectorAll('[data-dragent-ref]').forEach((el) => el.removeAttribute('data-dragent-ref'));
    const elements = []; let truncated = false; const seen = new Set();
    for (const { root, frame } of all) {
      for (const el of root.querySelectorAll(INTERACTIVE)) {
        if (seen.has(el) || !shown(el)) continue;
        seen.add(el);
        if (elements.length >= LIMIT) { truncated = true; break; }
        const ref = nextRef();
        el.setAttribute('data-dragent-ref', ref);
        elements.push(describe(el, ref, frame));
      }
    }
    const visible = (selector) => all.flatMap(({ root }) => [...root.querySelectorAll(selector)]).filter(shown);
    const headings = visible('h1,h2,h3').slice(0, 30).map((h) => `${h.tagName.toLowerCase()}: ${clean(h.innerText).slice(0, 120)}`);
    const dialogsOpen = visible('dialog[open],[role=dialog],[role=alertdialog],[aria-modal=true]').slice(0, 5)
      .map((d) => clean(d.getAttribute('aria-label') || (d.querySelector('h1,h2,h3') || {}).innerText || d.innerText).slice(0, 160));
    const alerts = visible('[role=alert],[role=status],[aria-live=assertive],[aria-live=polite],output')
      .map((a) => clean(a.innerText)).filter(Boolean).slice(0, 10).map((t) => t.slice(0, 200));
    const frames = [...document.querySelectorAll('iframe,frame')].filter((f) => { try { return !f.contentDocument; } catch (_) { return true; } })
      .slice(0, 10).map((f) => `${f.getAttribute('title') || f.getAttribute('name') || 'frame'}: ${f.src || '(no address)'} (another site: open its address to work in it)`);
    const drawn = visible('canvas').filter((c) => c.width >= 40 && c.height >= 40).slice(0, 5)
      .map((c) => `canvas ${Math.round(c.getBoundingClientRect().width)}×${Math.round(c.getBoundingClientRect().height)} (drawn content: read it from a screenshot)`);
    const body = document.body ? document.body.innerText : '';
    const text = body.replace(/[ \t]+/g, ' ').replace(/\n\s*\n+/g, '\n').trim().slice(0, 4000);
    return done({ title: document.title, url: location.href, headings, dialogs: dialogsOpen, alerts, frames, drawn, elements, truncated, text });
  };

  // ── Acting on elements ───────────────────────────────────────────────────
  // Dialogs a step raised, said in its answer.
  const during = (step) => {
    const before = dialogs.length;
    const result = step();
    const raised = dialogs.slice(before).map((d) => ` A ${d.kind} dialog appeared: “${d.message}” (${d.outcome}).`).join('');
    return raised ? result + raised : result;
  };
  const pointAt = (el) => {
    const r = el.getBoundingClientRect();
    return { bubbles: true, cancelable: true, composed: true, view: el.ownerDocument.defaultView, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, button: 0 };
  };
  const covering = (el) => {
    const at = pointAt(el);
    const top = el.ownerDocument.elementFromPoint(at.clientX, at.clientY);
    if (!top || top === el || el.contains(top) || top.contains(el)) return '';
    return ` (another element, ${top.tagName.toLowerCase()} "${clean(top.innerText || top.getAttribute('aria-label')).slice(0, 40)}", was on top of it)`;
  };

  const click = (ref) => {
    const el = find(ref);
    if (!el || !el.isConnected) return stale(ref);
    if (disabled(el)) return fail('element_disabled', `${ref} is disabled: something else on the page has to happen first (fill a required field, accept terms…). Snapshot again to see what.`);
    el.scrollIntoView({ block: 'center', inline: 'center' });
    const note = covering(el);
    const at = pointAt(el);
    return done(during(() => {
      el.dispatchEvent(new PointerEvent('pointerdown', { ...at, pointerType: 'mouse', isPrimary: true }));
      el.dispatchEvent(new MouseEvent('mousedown', at));
      if (typeof el.focus === 'function') el.focus({ preventScroll: true });
      el.dispatchEvent(new PointerEvent('pointerup', { ...at, pointerType: 'mouse', isPrimary: true }));
      el.dispatchEvent(new MouseEvent('mouseup', at));
      el.click();
      return `Clicked ${ref}${note}.`;
    }));
  };

  const hover = (ref) => {
    const el = find(ref);
    if (!el || !el.isConnected) return stale(ref);
    el.scrollIntoView({ block: 'center', inline: 'center' });
    const at = pointAt(el);
    for (const type of ['pointerover', 'pointerenter', 'mouseover', 'mouseenter', 'pointermove', 'mousemove']) {
      const Kind = type.startsWith('pointer') ? PointerEvent : MouseEvent;
      el.dispatchEvent(new Kind(type, { ...at, bubbles: !type.endsWith('enter'), pointerType: 'mouse' }));
    }
    return done(`Hovered over ${ref}. Snapshot again to see a menu or tooltip it opened.`);
  };

  const type = (ref, text, submit) => {
    const el = find(ref);
    if (!el || !el.isConnected) return stale(ref);
    if (el.type === 'file') return fail('unsupported', `${ref} is a file picker: pages can't be given files from a script. Ask the person to choose the file in the browser pane.`);
    if (disabled(el) || el.readOnly) return fail('element_disabled', `${ref} can't be typed into (disabled or read-only).`);
    el.scrollIntoView({ block: 'center' });
    el.focus();
    if (el.isContentEditable) {
      el.ownerDocument.execCommand('selectAll', false);
      el.ownerDocument.execCommand('insertText', false, text);
    } else if (el.tagName === 'SELECT') {
      const option = [...el.options].find((o) => o.value === text || clean(o.text) === clean(text));
      if (!option) return fail('no_such_option', `${ref} has no option “${text}”: its options are ${[...el.options].map((o) => `“${clean(o.text)}”`).join(', ')}.`);
      el.value = option.value;
    } else {
      const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, text);
    }
    el.dataset.dragentTouched = '1';
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return done(during(() => {
      if (submit) {
        const options = { key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true };
        const proceed = el.dispatchEvent(new KeyboardEvent('keydown', options));
        el.dispatchEvent(new KeyboardEvent('keyup', options));
        if (proceed && el.form) { el.form.requestSubmit ? el.form.requestSubmit() : el.form.submit(); }
      }
      const invalid = el.validity && !el.validity.valid ? ` The field reports: ${clean(el.validationMessage)}.` : '';
      return (submit ? `Typed into ${ref} and submitted.` : `Typed into ${ref}.`) + invalid;
    }));
  };

  const FOCUSABLE = 'a[href],button,input:not([type=hidden]),textarea,select,[tabindex]:not([tabindex="-1"]),[contenteditable=""],[contenteditable=true]';
  const press = (combo) => {
    const parts = String(combo).split('+').map((p) => p.trim()).filter(Boolean);
    let key = parts.pop() || '';
    const mods = new Set(parts.map((p) => p.toLowerCase()));
    if (key === 'Space') key = ' ';
    const codes = { Enter: 13, Escape: 27, Tab: 9, Backspace: 8, Delete: 46, ArrowUp: 38, ArrowDown: 40, ArrowLeft: 37, ArrowRight: 39, ' ': 32, Home: 36, End: 35, PageUp: 33, PageDown: 34 };
    const target = document.activeElement || document.body;
    const options = { key, code: key.length === 1 ? `Key${key.toUpperCase()}` : key, keyCode: codes[key] || (key.length === 1 ? key.toUpperCase().charCodeAt(0) : 0), bubbles: true, cancelable: true,
      ctrlKey: mods.has('control') || mods.has('ctrl'), shiftKey: mods.has('shift'), altKey: mods.has('alt') || mods.has('option'), metaKey: mods.has('meta') || mods.has('cmd') || mods.has('command') };
    options.which = options.keyCode;
    return done(during(() => {
      const proceed = target.dispatchEvent(new KeyboardEvent('keydown', options));
      target.dispatchEvent(new KeyboardEvent('keyup', options));
      if (proceed && key === 'Tab') {
        // A script's Tab doesn't move focus by itself: move it as a person's would.
        const order = [...document.querySelectorAll(FOCUSABLE)].filter((el) => shown(el) && !disabled(el));
        const at = order.indexOf(target);
        const next = order[(at + (options.shiftKey ? -1 : 1) + order.length) % order.length];
        if (next) next.focus();
      }
      if (proceed && key === 'Enter' && target.form && !options.ctrlKey && !options.metaKey) { target.form.requestSubmit ? target.form.requestSubmit() : target.form.submit(); }
      if (proceed && key === 'Enter' && target.tagName === 'A') target.click();
      const now = document.activeElement;
      const focus = now && now !== document.body ? ` Focus is on ${now.getAttribute('data-dragent-ref') || now.tagName.toLowerCase()} "${nameOf(now)}".` : '';
      return `Pressed ${combo}.${focus}`;
    }));
  };

  const scroll = (to) => {
    if (/^e\d+$/.test(to)) {
      const el = find(to);
      if (!el || !el.isConnected) return stale(to);
      el.scrollIntoView({ block: 'center' });
      return done(`Scrolled ${to} into view.`);
    }
    const page = document.scrollingElement || document.documentElement;
    if (to === 'top') page.scrollTo({ top: 0 });
    else if (to === 'bottom') page.scrollTo({ top: page.scrollHeight });
    else if (to === 'up' || to === 'down' || !to) page.scrollBy({ top: (to === 'up' ? -1 : 1) * innerHeight * 0.8 });
    else return fail('invalid_input', `Scroll down, up, top, bottom, or to an element (e3), not “${to}”.`);
    const end = Math.max(1, page.scrollHeight - innerHeight);
    return done(`Scrolled ${to || 'down'}: ${Math.round(100 * page.scrollTop / end)}% down the page.`);
  };

  // ── Waiting ──────────────────────────────────────────────────────────────
  // Whether every condition holds now, and what's still missing.
  const check = (conditions) => {
    const body = () => (document.body ? document.body.innerText : '');
    const missing = [];
    if (conditions.text != null && !body().includes(conditions.text)) missing.push(`text “${conditions.text}” isn't on the page`);
    if (conditions.gone != null && body().includes(conditions.gone)) missing.push(`text “${conditions.gone}” is still on the page`);
    if (conditions.selector != null) {
      let present = false;
      try { present = roots().some(({ root }) => [...root.querySelectorAll(conditions.selector)].some(shown)); }
      catch (_) { return fail('invalid_input', `“${conditions.selector}” isn't a CSS selector.`); }
      if (!present) missing.push(`nothing visible matches ${conditions.selector}`);
    }
    if (conditions.ref_gone != null) {
      const el = find(conditions.ref_gone);
      if (el && el.isConnected && shown(el)) missing.push(`${conditions.ref_gone} is still shown`);
    }
    if (conditions.url != null && !location.href.includes(conditions.url)) missing.push(`the address is ${location.href}`);
    if (conditions.idle) {
      const quiet = Date.now() - lastActivity;
      if (document.readyState !== 'complete') missing.push('the page is still loading');
      else if (inflight > 0) missing.push(`${inflight} request${inflight === 1 ? '' : 's'} still running`);
      else if (quiet < 500) missing.push('the page is still changing');
    }
    return done({ met: missing.length === 0, detail: missing.length ? missing.join('; ') : 'All conditions hold.' });
  };

  // ── Reading what happened ────────────────────────────────────────────────
  const readConsole = (clear, only) => {
    const entries = only ? log.filter((entry) => only.includes(entry.level)) : log;
    const text = entries.map((entry) => `[${entry.level}] ${entry.text}`).join('\n');
    if (clear) log.length = 0;
    return done(text || (only ? `Nothing of that kind (${only.join(', ')}) was logged.` : 'The page hasn\'t logged anything since it loaded.'));
  };
  const dialog = (accept, text) => {
    nextAnswer = { accept: !!accept, text: text == null ? null : String(text) };
    return done(accept
      ? `The next dialog will be accepted${text != null ? ` with “${text}”` : ''}. Now do what opens it.`
      : 'The next dialog will be dismissed. Now do what opens it.');
  };

  // Scripts that return a promise: kept here until they settle.
  const pending = {};
  let pendingSeq = 0;
  // `thunk` is an inline script (one expression the desktop built into this
  // call): it runs even where the page's CSP forbids eval.
  const evaluate = (script, settle, thunk) => {
    try {
      const value = thunk ? thunk() : (0, eval)(script);
      if (value && typeof value.then === 'function') {
        if (!settle) return done('The script returned a promise. Run it with --await to wait for its value.');
        const id = ++pendingSeq;
        pending[id] = { settled: false };
        value.then((v) => { pending[id] = { settled: true, ok: render(v) }; }, (e) => { pending[id] = { settled: true, error: `The script failed: ${e && e.stack ? e.stack : e}` }; });
        return done({ pending: id });
      }
      return done(render(value));
    } catch (error) {
      if (!thunk && error && error.name === 'EvalError') {
        return fail('eval_blocked', 'This page\'s security policy (CSP) doesn\'t allow scripts given as text, so eval can\'t run here. Snapshot, click, type, press and scroll still work.');
      }
      return fail('script_error', `The script failed: ${error && error.stack ? error.stack : error}`);
    }
  };
  const render = (value) => {
    let text;
    if (value === undefined) text = 'undefined';
    else if (typeof value === 'string') text = value;
    else if (value instanceof Element) text = value.outerHTML.slice(0, 4000);
    else { try { text = JSON.stringify(value, null, 2); } catch (_) { text = String(value); } }
    return String(text).slice(0, 20000);
  };
  const settled = (id) => {
    const entry = pending[id];
    if (!entry) return fail('stale_ref', 'That script\'s result is gone: the page reloaded or navigated.');
    if (!entry.settled) return done({ met: false, detail: 'the promise hasn\'t settled' });
    delete pending[id];
    if (entry.error) return fail('script_error', entry.error);
    return done({ met: true, detail: entry.ok });
  };

  window.__dragentAgent = { snapshot, click, hover, type, press, scroll, check, readConsole, dialog, evaluate, settled };
})();
