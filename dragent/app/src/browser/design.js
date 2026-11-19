/* Loaded once per document. No observers, timers, or pointer listeners until design mode opens. */
(() => {
    if (window.__dragentDesign) return;
    // mode: 'pick' (hovering), 'card' (writing a note), 'draw' (marking up a
    // screenshot), 'busy' (a capture is being taken; the overlay is hidden).
    let config = null, host = null, root = null, mode = 'pick', hover = null;
    let frame = 0, editTimer = 0, pendingEdit = null, captureTimer = 0, toastTimer = 0;
    let lastPointer = null, pendingPointer = null, drawing = null, destroyed = false, steered = false;
    let card = null, cardIsNew = false, cardError = '', awaiting = false, trayOpen = false;
    let toast = '', known = new Set();
    const maxNotes = 20;
    const mac = /Mac|iPhone|iPad/.test(navigator.platform);
    const sendKeys = mac ? '⌘↵' : 'Ctrl+↵';
    const post = (kind, data = {}) => {
        if (config) window.ipc.postMessage('dragent-design:' + JSON.stringify({ token: config.token, kind, ...data }));
    };
    const el = (tag, text, attrs = {}) => {
        const node = document.createElement(tag);
        if (text !== undefined) node.textContent = text;
        for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, value);
        return node;
    };
    const button = (text, title, run, className = 'btn') => {
        const b = el('button', text, { type: 'button', title, 'aria-label': title, class: className });
        b.addEventListener('click', run);
        return b;
    };
    const clip = (s, n) => String(s || '').slice(0, n);
    const text = (node, n = 1000) => clip(node?.innerText ?? node?.textContent, n).replace(/\s+/g, ' ').trim();
    const viewport = () => ({ x: scrollX, y: scrollY, width: innerWidth, height: innerHeight });
    const cssEscape = value => CSS.escape(value);
    const current = () => config?.notes.find(n => n.id === card);
    function selector(target) {
        const parts = [];
        let node = target;
        for (let depth = 0; node && depth < 10; depth++) {
            if (node.id) { parts.unshift('#' + cssEscape(node.id)); break; }
            const testId = node.getAttribute('data-testid');
            let segment = node.localName;
            if (testId) segment += '[data-testid=' + JSON.stringify(testId) + ']';
            else if (node.parentElement) {
                const siblings = [...node.parentElement.children].filter(n => n.localName === node.localName);
                if (siblings.length > 1) segment += ':nth-of-type(' + (siblings.indexOf(node) + 1) + ')';
            }
            parts.unshift(segment);
            if (node.parentElement) node = node.parentElement;
            else {
                const tree = node.getRootNode();
                if (tree.host) { parts.unshift('>>>'); node = tree.host; }
                else break;
            }
        }
        return clip(parts.join(' > ').replace(/ > >>> > /g, ' >>> '), 3500);
    }
    // A readable ancestor chain: `main > section.card > button#upgrade`.
    function path(target) {
        const parts = [];
        for (let node = target, depth = 0; node instanceof Element && depth < 6; node = node.parentElement, depth++) {
            if (node === document.body || node === document.documentElement) break;
            const classes = [...node.classList].filter(c => /^[a-zA-Z][\w-]{1,30}$/.test(c) && !/\d{3}/.test(c)).slice(0, 2);
            parts.unshift(node.localName + (node.id ? '#' + node.id : '') + classes.map(c => '.' + c).join(''));
            if (node.id && depth > 0) break;
        }
        return clip(parts.join(' > '), 1500);
    }
    const implicitRoles = {
        a: 'link', button: 'button', textarea: 'textbox', select: 'combobox', img: 'img',
        nav: 'navigation', main: 'main', header: 'banner', footer: 'contentinfo', aside: 'complementary',
        form: 'form', dialog: 'dialog', ul: 'list', ol: 'list', li: 'listitem', table: 'table',
        h1: 'heading', h2: 'heading', h3: 'heading', h4: 'heading', h5: 'heading', h6: 'heading'
    };
    function role(target) {
        const explicit = target.getAttribute('role');
        if (explicit) return clip(explicit, 100);
        if (target.localName === 'input') {
            return ({ checkbox: 'checkbox', radio: 'radio', button: 'button', submit: 'button', range: 'slider' })[target.type] || 'textbox';
        }
        return implicitRoles[target.localName] || '';
    }
    function accessibleName(target) {
        const labelledBy = target.getAttribute('aria-labelledby');
        if (labelledBy) {
            const named = labelledBy.split(/\s+/).map(id => text(document.getElementById(id), 200)).join(' ').trim();
            if (named) return clip(named, 500);
        }
        return clip(target.getAttribute('aria-label') || target.getAttribute('alt') || target.getAttribute('title')
            || target.getAttribute('placeholder') || (target.labels?.[0] ? text(target.labels[0], 200) : ''), 500);
    }
    function domSnippet(target) {
        let remaining = 16000, nodes = 0;
        const escape = value => String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');
        const walk = (node, depth) => {
            if (remaining <= 0 || ++nodes > 80) return '';
            if (node.nodeType === Node.TEXT_NODE) {
                const result = escape(clip(node.textContent, Math.min(remaining, 500)));
                remaining -= result.length;
                return result;
            }
            if (!(node instanceof Element) || /^(script|style|noscript)$/.test(node.localName)) return '';
            const attrs = [...node.attributes].filter(a => /^(id|class|role|aria-label|data-testid|type|name|href|src|alt)$/.test(a.name))
                .slice(0, 12).map(a => ' ' + a.name + '="' + escape(clip(a.value, 180)) + '"').join('');
            let result = '<' + node.localName + attrs + '>';
            remaining -= result.length;
            if (depth < 4 && !/^(input|textarea)$/.test(node.localName)) {
                for (const child of node.childNodes) {
                    if (remaining <= 0 || nodes > 80) break;
                    result += walk(child, depth + 1);
                }
            }
            return result + '</' + node.localName + '>';
        };
        return clip(walk(target, 0), 18000);
    }
    // React 19 keeps the JSX call site as a stack; the first frame outside the
    // runtime is the component that rendered the element.
    function stackSource(stack) {
        for (const line of String(stack || '').split('\n').slice(1)) {
            const match = line.match(/((?:https?|file):\/\/[^\s)]+?):(\d+):\d+/);
            if (!match || /node_modules|react-dom|react-jsx|react\.development|\/@vite\/|\/@react-refresh|chunk-/.test(match[1])) continue;
            try { return decodeURIComponent(new URL(match[1]).pathname).replace(/^\/@fs/, '') + ':' + match[2]; } catch (_) { }
        }
        return '';
    }
    function sourceHint(target) {
        const names = [], seen = new Set();
        let source = '';
        for (let node = target, depth = 0; node && depth < 8; node = node.parentElement, depth++) {
            source ||= node.getAttribute('data-source') || node.getAttribute('data-source-file') || node.getAttribute('data-sentry-source-file') || '';
            const inspector = node.getAttribute('data-inspector-relative-path');
            if (inspector) source ||= inspector + ':' + (node.getAttribute('data-inspector-line') || 1);
            const svelte = node.__svelte_meta?.loc;
            if (svelte?.file) source ||= svelte.file + ':' + (svelte.line + 1);
            const vue = node.__vueParentComponent;
            if (vue?.type) {
                source ||= vue.type.__file || '';
                const name = vue.type.name || vue.type.__name;
                if (name && !seen.has(name)) { names.push(name); seen.add(name); }
            }
            const key = Object.keys(node).find(k => k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$'));
            let fiber = key && node[key];
            for (let i = 0; fiber && i < 14; i++, fiber = fiber.return) {
                const info = fiber._debugSource || fiber._debugOwner?._debugSource;
                if (info?.fileName) source ||= info.fileName + ':' + (info.lineNumber || 1);
                if (!source && fiber._debugStack) source = stackSource(fiber._debugStack.stack);
                const type = fiber.type;
                const name = typeof type === 'function' ? type.displayName || type.name : type?.displayName;
                if (name && !seen.has(name)) { names.push(name); seen.add(name); }
            }
        }
        return { source: clip(source, 2000), components: clip(names.join(' → '), 1500) };
    }
    function describe(target) {
        const r = target.getBoundingClientRect(), style = getComputedStyle(target), styles = {};
        for (const property of ['display', 'position', 'width', 'height', 'margin', 'padding', 'gap', 'color',
            'background-color', 'border', 'border-radius', 'box-shadow', 'font-family', 'font-size', 'font-weight',
            'line-height', 'text-align', 'align-items', 'justify-content', 'flex-direction', 'grid-template-columns', 'opacity', 'z-index']) {
            styles[property] = clip(style.getPropertyValue(property), 800);
        }
        return {
            url: clip(location.href, 8000), title: clip(document.title, 500), selector: selector(target),
            tag: target.localName, text: clip(target.getAttribute('aria-label') || text(target), 2500),
            html: domSnippet(target), nearby: clip([text(target.previousElementSibling, 500), text(target.parentElement, 1800),
            text(target.nextElementSibling, 500)].filter(Boolean).join(' | '), 3000),
            role: role(target), name: accessibleName(target), path: path(target),
            ...sourceHint(target), styles, rect: { x: r.x, y: r.y, width: r.width, height: r.height }, viewport: viewport()
        };
    }
    const component = selection => selection.components?.split(' → ')[0] || '';
    // `PlanCard`, `button "Upgrade now"`, or `<div>`.
    function shortLabel(note) {
        if (note.markup) return 'Screenshot';
        const s = note.selection, name = clip((s.name || s.text || '').replace(/\s+/g, ' ').trim(), 28);
        return component(s) || (name ? s.tag + ' "' + name + '"' : '<' + s.tag + '>');
    }
    const ownEvent = event => event.composedPath().includes(host);
    function flushEdit() {
        clearTimeout(editTimer);
        if (pendingEdit) { post('update', pendingEdit); pendingEdit = null; }
    }
    function mount() {
        destroyed = false;
        host = el('div', undefined, { 'data-dragent-design': '' });
        host.style.cssText = 'all:initial!important;position:fixed!important;inset:0!important;z-index:2147483647!important;pointer-events:none!important;';
        root = host.attachShadow({ mode: 'closed' });
        const style = el('style');
        // `all:initial` on the host keeps page styles out, and also wins over
        // `:host` rules, so text styles live on `.dd` inside the shadow root.
        style.textContent = `
      .dd{position:fixed;inset:0;pointer-events:none;font:12.5px/1.45 system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Ubuntu,"Helvetica Neue",sans-serif;color:var(--fg);text-align:left;letter-spacing:normal;-webkit-font-smoothing:antialiased}
      *{box-sizing:border-box}button,input,textarea,select{font:inherit;color:inherit;margin:0}button,select{cursor:pointer}
      .btn{display:inline-flex;align-items:center;gap:5px;height:26px;padding:0 9px;border-radius:6px;border:1px solid var(--line);background:var(--raised);color:var(--fg);white-space:nowrap}
      .btn:hover{border-color:var(--muted)}.btn.ghost{background:transparent;border-color:transparent;color:var(--muted)}
      .btn.ghost:hover{color:var(--fg);background:var(--raised)}.btn.ghost.danger:hover{color:var(--danger)}
      .btn.primary{background:var(--accent);border-color:var(--accent);color:var(--accent-fg);font-weight:600}.btn.primary:hover{filter:brightness(1.08)}
      .btn[aria-pressed=true]{background:var(--accent);border-color:var(--accent);color:var(--accent-fg)}
      .btn:disabled{opacity:.45;cursor:default;filter:none}:focus-visible{outline:2px solid var(--accent);outline-offset:1px}
      .panel{position:fixed;pointer-events:auto;background:var(--bg);border:1px solid var(--line);border-radius:10px;box-shadow:0 10px 30px rgba(0,0,0,.28),0 2px 6px rgba(0,0,0,.16)}
      .glass{position:fixed;inset:0;pointer-events:auto;cursor:crosshair;z-index:0}.outline,.tag{z-index:1}.pin{z-index:2}.panel{z-index:3}.drawing{z-index:4}
      .outline{position:fixed;pointer-events:none;border:2px solid var(--accent);border-radius:3px;background:color-mix(in srgb,var(--accent) 12%,transparent);box-shadow:0 0 0 1px rgba(0,0,0,.35),inset 0 0 0 1px rgba(255,255,255,.45)}
      .outline.selected{background:transparent}
      .tag{position:fixed;pointer-events:none;max-width:min(420px,calc(100vw - 16px));background:var(--accent);color:var(--accent-fg);padding:2px 7px;border-radius:5px;font-size:11.5px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;box-shadow:0 2px 6px rgba(0,0,0,.25)}
      .tag span{font-weight:400;opacity:.85}
      .pin{position:fixed;pointer-events:auto;width:22px;height:22px;border-radius:50% 50% 50% 3px;background:var(--accent);color:var(--accent-fg);border:2px solid var(--bg);font-size:11px;font-weight:700;display:flex;align-items:center;justify-content:center;padding:0;box-shadow:0 2px 6px rgba(0,0,0,.3)}
      .pin.empty{background:var(--bg);color:var(--fg);border-color:var(--accent)}
      .card{width:min(360px,calc(100vw - 16px));padding:10px}
      .head{display:flex;align-items:center;gap:6px;margin-bottom:7px}.spacer{flex:1}
      .chip{display:inline-flex;align-items:center;gap:5px;min-width:0;max-width:240px;padding:2px 8px;border-radius:5px;background:color-mix(in srgb,var(--accent) 18%,transparent);font-weight:600;font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
      .chip b{color:var(--accent);font-weight:700}
      select.small{height:24px;border-radius:6px;border:1px solid var(--line);background:var(--raised);padding:0 4px;font-size:12px}
      textarea{display:block;width:100%;min-height:52px;max-height:160px;resize:none;border:1px solid var(--line);border-radius:7px;background:var(--raised);padding:7px 8px;line-height:1.4;outline:none}
      textarea:focus{border-color:var(--accent)}textarea::placeholder,input::placeholder{color:var(--muted)}
      .foot{display:flex;align-items:center;gap:2px;margin-top:8px}
      .note{color:var(--muted);font-size:11px;margin-top:7px}.error{color:var(--danger);font-size:11.5px;margin-top:6px}
      .thumb{display:block;max-width:100%;max-height:120px;border-radius:6px;border:1px solid var(--line);margin-bottom:7px}
      .pill{left:50%;bottom:12px;transform:translateX(-50%);padding:6px 12px;border-radius:999px;font-size:12px;white-space:nowrap;max-width:calc(100vw - 24px);overflow:hidden;text-overflow:ellipsis}
      .pill.hint{pointer-events:none;color:var(--muted);max-width:calc(100vw - 260px)}.panel.pill{z-index:6}
      .notes{right:12px;bottom:12px;display:flex;align-items:center;gap:7px;height:30px;padding:0 12px;border-radius:999px;font-weight:600}
      .count{min-width:18px;height:18px;padding:0 5px;border-radius:9px;background:var(--accent);color:var(--accent-fg);font-size:10.5px;font-weight:700;display:inline-flex;align-items:center;justify-content:center}
      .tray{right:12px;bottom:12px;width:min(340px,calc(100vw - 24px));max-height:min(440px,calc(100vh - 24px));display:flex;flex-direction:column;overflow:hidden}
      .tray header{display:flex;align-items:center;gap:7px;padding:7px 8px 7px 12px;border-bottom:1px solid var(--line);font-weight:600}
      .tray ol{list-style:none;margin:0;padding:4px;overflow:auto}.tray li{display:flex;align-items:flex-start;gap:4px;border-radius:7px}
      .tray li:hover{background:var(--raised)}.tray footer{display:flex;align-items:center;gap:4px;padding:7px 8px;border-top:1px solid var(--line)}
      .row{flex:1;min-width:0;display:flex;gap:8px;align-items:flex-start;text-align:left;background:none;border:0;padding:6px}
      .row .count{flex:none;margin-top:1px}.row div{min-width:0}.what,.said{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
      .what{font-weight:600}.said{color:var(--muted)}.said.missing{color:var(--warning)}
      .drawing{position:fixed;inset:0;background:rgba(0,0,0,.72);pointer-events:auto;display:flex;flex-direction:column;align-items:center;gap:10px;padding:10px 12px 14px}
      .stage{flex:1;min-height:0;width:100%;display:flex;align-items:center;justify-content:center}
      canvas{max-width:100%;max-height:100%;touch-action:none;cursor:crosshair;border-radius:6px;box-shadow:0 8px 30px rgba(0,0,0,.5)}
      .drawing .tools{position:static;flex:none;display:flex;flex-wrap:wrap;justify-content:center;gap:3px;padding:5px;align-items:center;max-width:100%}
      .sep{flex:none;width:1px;height:18px;background:var(--line);margin:0 3px}
      .swatch{flex:none;width:20px;height:20px;border-radius:50%;border:2px solid var(--bg);box-shadow:0 0 0 1px var(--line);padding:0}
      .swatch[aria-pressed=true]{box-shadow:0 0 0 2px var(--fg)}
      .tools input[type=text]{height:26px;width:120px;border:1px solid var(--line);border-radius:6px;background:var(--raised);padding:0 7px}
      @media(prefers-reduced-motion:no-preference){.btn{transition:background .1s,border-color .1s}}
    `;
        const wrap = el('div', undefined, { class: 'dd' });
        wrap.append(el('div', undefined, { id: 'geo' }), el('div', undefined, { id: 'ui' }));
        root.append(style, wrap);
        document.documentElement.append(host);
        // A closed shadow root hides its nodes from document listeners (they see
        // only the host), so pointer work on the glass is handled in here.
        wrap.addEventListener('pointermove', event => onGlass(event) && pointerMove(event));
        wrap.addEventListener('pointerdown', event => onGlass(event) && block(event));
        wrap.addEventListener('pointerup', event => onGlass(event) && block(event));
        wrap.addEventListener('click', event => onGlass(event) && pick(event));
        wrap.addEventListener('keydown', fieldKeydown);
        document.addEventListener('pointermove', pagePointerMove, true);
        document.addEventListener('pointerdown', pageBlock, true);
        document.addEventListener('pointerup', pageBlock, true);
        document.addEventListener('click', pagePick, true);
        document.addEventListener('keydown', keydown, true);
        addEventListener('scroll', updateGeometry, true);
        addEventListener('resize', updateGeometry);
    }
    function unmount() {
        flushEdit(); destroyed = true;
        cancelAnimationFrame(frame); frame = 0;
        clearTimeout(captureTimer); clearTimeout(toastTimer);
        document.removeEventListener('pointermove', pagePointerMove, true);
        document.removeEventListener('pointerdown', pageBlock, true);
        document.removeEventListener('pointerup', pageBlock, true);
        document.removeEventListener('click', pagePick, true);
        document.removeEventListener('keydown', keydown, true);
        removeEventListener('scroll', updateGeometry, true);
        removeEventListener('resize', updateGeometry);
        host?.remove(); host = root = hover = drawing = card = null;
        mode = 'pick'; awaiting = trayOpen = false; toast = cardError = '';
    }
    const onGlass = event => event.target?.classList?.contains('glass');
    // Events aimed at the page itself, e.g. from a top-layer dialog above the glass.
    const pagePointerMove = event => ownEvent(event) || pointerMove(event);
    const pageBlock = event => ownEvent(event) || block(event);
    const pagePick = event => ownEvent(event) || pick(event);
    function elementAt(x, y) {
        const glass = root?.querySelector('.glass');
        if (glass) glass.style.pointerEvents = 'none';
        let target = document.elementFromPoint(x, y);
        for (let depth = 0; target?.shadowRoot && depth < 8; depth++) {
            const inner = target.shadowRoot.elementFromPoint(x, y);
            if (!inner || inner === target) break;
            target = inner;
        }
        if (glass) glass.style.pointerEvents = '';
        return target === host ? null : target;
    }
    // While picking or writing a note, the page doesn't react to the pointer.
    function block(event) {
        if (mode === 'pick' || mode === 'card') { event.preventDefault(); event.stopImmediatePropagation(); }
    }
    function pointerMove(event) {
        if (mode !== 'pick') return;
        if (lastPointer?.x === event.clientX && lastPointer?.y === event.clientY) return;
        lastPointer = { x: event.clientX, y: event.clientY };
        pendingPointer = lastPointer; steered = false;
        updateGeometry();
    }
    function updateGeometry() {
        if (!root || frame) return;
        frame = requestAnimationFrame(() => {
            frame = 0;
            if (root) {
                if (pendingPointer && mode === 'pick') hover = elementAt(pendingPointer.x, pendingPointer.y);
                pendingPointer = null;
                geometry();
            }
        });
    }
    // Where a note's element is now, in viewport coordinates.
    function noteRect(note) {
        if (note.markup || note.selection.url !== location.href) return null;
        try {
            const target = document.querySelector(note.selection.selector);
            if (target) return target.getBoundingClientRect();
        } catch (_) { }
        const r = note.selection.rect, v = note.selection.viewport;
        return { x: r.x + v.x - scrollX, y: r.y + v.y - scrollY, width: r.width, height: r.height };
    }
    function box(r, className) {
        const node = el('div', undefined, { class: className });
        Object.assign(node.style, { left: r.x + 'px', top: r.y + 'px', width: r.width + 'px', height: r.height + 'px' });
        return node;
    }
    function geometry() {
        const layer = root.getElementById('geo');
        layer.replaceChildren();
        if (!config || drawing || mode === 'busy') return;
        if (mode === 'pick' && hover?.isConnected) {
            const r = hover.getBoundingClientRect(), hint = sourceHint(hover);
            const tag = el('div', undefined, { class: 'tag' });
            tag.append(hover.localName + (hover.id ? '#' + hover.id : ''),
                el('span', (hint.components ? ' · ' + hint.components.split(' → ')[0] : '') + ' · ' + Math.round(r.width) + ' × ' + Math.round(r.height)));
            Object.assign(tag.style, {
                left: Math.max(4, Math.min(innerWidth - 240, r.x)) + 'px',
                top: (r.y > 26 ? r.y - 25 : Math.min(innerHeight - 24, r.y + r.height + 4)) + 'px'
            });
            layer.append(box(r, 'outline'), tag);
        }
        const note = current();
        const r = note && noteRect(note);
        if (mode === 'card' && r) layer.append(box(r, 'outline selected'));
        for (const [index, item] of config.notes.entries()) {
            const at = noteRect(item);
            if (!at || at.x < -24 || at.y < -24 || at.x > innerWidth || at.y > innerHeight) continue;
            const pin = button(String(index + 1), 'Note ' + (index + 1) + ': ' + (item.comment || 'no request yet'), () => openNote(item.id),
                'pin' + (item.comment.trim() ? '' : ' empty'));
            Object.assign(pin.style, { left: Math.max(2, at.x - 11) + 'px', top: Math.max(2, at.y - 11) + 'px' });
            layer.append(pin);
        }
        placeCard();
    }
    // Below the element when it fits, else above, else inside the viewport.
    function placeCard() {
        const panel = root.querySelector('.card');
        const note = current();
        if (!panel || !note) return;
        const width = panel.offsetWidth, height = panel.offsetHeight, r = noteRect(note);
        let left = (innerWidth - width) / 2, top = innerHeight - height - 56;
        if (r) {
            left = r.x;
            top = r.y + r.height + 10;
            if (top + height > innerHeight - 8) top = r.y - height - 10;
            if (top < 8) top = innerHeight - height - 8;
        }
        panel.style.left = Math.max(8, Math.min(innerWidth - width - 8, left)) + 'px';
        panel.style.top = Math.max(8, top) + 'px';
    }
    function capture(kind, data) {
        flushEdit(); hover = null; card = null; mode = 'busy'; awaiting = true;
        root.getElementById('geo').replaceChildren();
        host.style.setProperty('visibility', 'hidden', 'important');
        requestAnimationFrame(() => requestAnimationFrame(() => {
            if (destroyed) return;
            post(kind, data);
            captureTimer = setTimeout(() => api.error('The capture timed out. Try again.'), 9000);
        }));
    }
    function pick(event) {
        if (mode === 'card') { event.preventDefault(); event.stopImmediatePropagation(); closeCard(); return; }
        if (mode !== 'pick') return;
        event.preventDefault(); event.stopImmediatePropagation();
        const target = (steered && hover) || elementAt(event.clientX, event.clientY) || hover;
        if (!target || !target.isConnected) return;
        if (config.notes.length >= maxNotes) { say('Keep up to 20 notes at a time. Send or delete some first.'); return; }
        capture('pick', { selection: describe(target) });
    }
    function draw() {
        if (mode === 'busy' || drawing) return;
        if (config.notes.length >= maxNotes) { say('Keep up to 20 notes at a time. Send or delete some first.'); return; }
        capture('capture_markup', {
            selection: {
                ...describe(document.documentElement), tag: 'screenshot', selector: 'viewport',
                rect: { x: 0, y: 0, width: innerWidth, height: innerHeight }
            }
        });
    }
    function openNote(id) {
        flushEdit();
        if (!config.notes.some(n => n.id === id)) return;
        card = id; cardIsNew = false; cardError = ''; mode = 'card'; trayOpen = false; hover = null;
        const note = current();
        if (!note.markup && note.selection.url === location.href) {
            try { document.querySelector(note.selection.selector)?.scrollIntoView({ block: 'nearest', inline: 'nearest' }); } catch (_) { }
        }
        render();
        root.querySelector('.card textarea')?.focus({ preventScroll: true });
    }
    // A new note nobody wrote anything for is discarded; anything typed is kept.
    function closeCard() {
        flushEdit();
        const note = current();
        if (note && cardIsNew && !note.comment.trim()) post('remove', { id: note.id });
        card = null; cardIsNew = false; cardError = ''; mode = 'pick';
        render();
    }
    function save() {
        flushEdit();
        const note = current();
        if (note && !note.comment.trim()) { cardError = 'Describe the change first.'; render(); return; }
        card = null; cardIsNew = false; cardError = ''; mode = 'pick';
        render();
    }
    function say(message) {
        clearTimeout(toastTimer);
        toast = clip(message, 600);
        toastTimer = setTimeout(() => { toast = ''; render(); }, 4000);
        render();
    }
    function keydown(event) {
        if (!config) return;
        const field = ownEvent(event) || event.composedPath().some(n => n instanceof HTMLElement && /^(TEXTAREA|INPUT|SELECT)$/.test(n.tagName));
        if (event.key === 'Escape') {
            event.preventDefault(); event.stopImmediatePropagation();
            if (drawing) { drawing = null; mode = 'pick'; render(); }
            else if (mode === 'card') closeCard();
            else if (trayOpen) { trayOpen = false; render(); }
            else if (mode === 'pick') { flushEdit(); post('close'); }
            return;
        }
        if (drawing && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'z' && root.activeElement?.type !== 'text') {
            event.preventDefault(); event.stopImmediatePropagation();
            if (event.shiftKey) redo(); else undo();
            return;
        }
        if (mode !== 'pick' || !hover || field) return;
        if (event.altKey && event.key === 'ArrowUp') {
            event.preventDefault(); hover = hover.parentElement || hover; steered = true; updateGeometry();
        } else if (event.altKey && event.key === 'ArrowDown') {
            event.preventDefault(); hover = hover.firstElementChild || hover; steered = true; updateGeometry();
        } else if (!event.metaKey && !event.ctrlKey && !event.altKey && event.key.toLowerCase() === 'c') {
            event.preventDefault(); event.stopImmediatePropagation();
            post('copy_element', { selection: describe(hover) });
        }
    }
    // Keys typed in the overlay's own fields.
    function fieldKeydown(event) {
        if (mode === 'card' && event.target.tagName === 'TEXTAREA' && event.key === 'Enter' && !event.shiftKey
            && !event.isComposing && !event.metaKey && !event.ctrlKey && !event.altKey) {
            event.preventDefault(); event.stopPropagation(); save();
        }
    }
    function render() {
        if (!root || !config) return;
        const ui = root.getElementById('ui');
        // Rebuilding must not take the caret away from someone typing.
        const active = root.activeElement, keep = active?.dataset?.keep;
        const caret = keep && active.selectionStart != null ? [active.selectionStart, active.selectionEnd] : null;
        ui.replaceChildren();
        if (drawing) renderDrawing(ui);
        else {
            if (mode === 'pick' || mode === 'card') ui.append(el('div', undefined, { class: 'glass' }));
            if (mode === 'card') renderCard(ui);
            if (config.notes.length && mode !== 'busy') ui.append(trayOpen ? renderTray() : renderNotesButton());
            if (toast) ui.append(el('div', toast, { class: 'panel pill', role: 'status' }));
            else if (mode === 'pick' && !trayOpen) {
                ui.append(el('div', 'Click any element to add a note · ' + (mac ? '⌥' : 'Alt+') + '↑ selects its parent · C copies it · Esc exits',
                    { class: 'panel pill hint' }));
            }
        }
        if (keep) {
            const again = root.querySelector('[data-keep="' + keep + '"]');
            if (again) { again.focus({ preventScroll: true }); if (caret && again.setSelectionRange) again.setSelectionRange(...caret); }
        }
        geometry();
    }
    function renderCard(ui) {
        const note = current();
        if (!note) { card = null; mode = 'pick'; return; }
        const panel = el('div', undefined, { class: 'panel card', role: 'dialog', 'aria-label': 'Design note' });
        const head = el('div', undefined, { class: 'head' }), chip = el('span', undefined, { class: 'chip', title: note.selection.selector });
        chip.append(el('b', note.markup ? '✎' : '⌖'), shortLabel(note));
        const intent = el('select', undefined, { class: 'small', 'aria-label': 'What kind of request' });
        for (const value of ['change', 'fix', 'question', 'remove']) intent.append(el('option', value[0].toUpperCase() + value.slice(1), { value }));
        intent.value = note.intent;
        head.append(chip, el('span', undefined, { class: 'spacer' }), intent);
        panel.append(head);
        if (note.markup && note.preview?.startsWith('data:image/')) panel.append(el('img', undefined, { class: 'thumb', src: note.preview, alt: 'Marked-up screenshot' }));
        const comment = el('textarea', undefined, {
            placeholder: note.markup ? 'What should change in this screenshot?' : 'Describe the change…',
            maxlength: '8192', rows: '2', 'aria-label': 'Requested change', 'data-keep': 'comment-' + note.id
        });
        comment.value = note.comment;
        const sending = config.sending;
        comment.disabled = intent.disabled = sending;
        const grow = () => { comment.style.height = 'auto'; comment.style.height = Math.min(comment.scrollHeight + 2, 160) + 'px'; };
        const changed = () => {
            note.comment = comment.value; note.intent = intent.value; cardError = '';
            pendingEdit = { id: note.id, comment: note.comment, intent: note.intent };
            clearTimeout(editTimer); editTimer = setTimeout(flushEdit, 350);
        };
        comment.addEventListener('input', () => { changed(); grow(); });
        comment.addEventListener('blur', flushEdit);
        intent.addEventListener('change', () => { changed(); flushEdit(); });
        panel.append(comment);
        if (cardError) panel.append(el('div', cardError, { class: 'error', role: 'alert' }));
        const foot = el('div', undefined, { class: 'foot' });
        if (cardIsNew && !note.markup) foot.append(button('Pick again', 'Discard this note and pick another element', () => {
            post('remove', { id: note.id }); card = null; cardIsNew = false; mode = 'pick'; render();
        }, 'btn ghost'));
        foot.append(button('Copy', 'Copy this note with the element’s HTML and CSS', () => { flushEdit(); post('copy_note', { id: note.id }); }, 'btn ghost'));
        if (note.screenshot) foot.append(button('Copy image', 'Copy the screenshot', () => post('copy_image', { id: note.id }), 'btn ghost'));
        if (!cardIsNew) foot.append(button('Delete', 'Delete this note', () => {
            post('remove', { id: note.id }); card = null; mode = 'pick'; render();
        }, 'btn ghost danger'));
        const done = button(cardIsNew ? 'Add note' : 'Done', 'Save this note · Enter', save, 'btn primary');
        done.disabled = sending;
        foot.append(el('span', undefined, { class: 'spacer' }), done);
        panel.append(foot, el('div', (cardIsNew ? 'Enter adds it · ' : 'Enter saves · ') + sendKeys + ' sends '
            + (config.notes.length === 1 ? 'it' : 'all ' + config.notes.length) + ' to ' + (config.target || 'an agent (choose with Send ▾)') + ' · Esc closes',
            { class: 'note' }));
        if (!note.screenshot && note.capture_error) panel.append(el('div', 'No screenshot: ' + note.capture_error, { class: 'note' }));
        ui.append(panel);
        requestAnimationFrame(grow);
    }
    function renderNotesButton() {
        const count = config.notes.length;
        const b = button('', 'Review your notes', () => { flushEdit(); trayOpen = true; render(); }, 'panel btn notes');
        b.append(el('span', String(count), { class: 'count' }), count === 1 ? 'note' : 'notes');
        return b;
    }
    function renderTray() {
        const tray = el('section', undefined, { class: 'panel tray', 'aria-label': 'Design notes' });
        const header = el('header');
        header.append(el('span', 'Notes'), el('span', String(config.notes.length), { class: 'count' }), el('span', undefined, { class: 'spacer' }),
            button('−', 'Hide notes', () => { trayOpen = false; render(); }, 'btn ghost'));
        const list = el('ol');
        for (const [index, note] of config.notes.entries()) {
            const item = el('li'), row = button('', 'Open note ' + (index + 1), () => openNote(note.id), 'row');
            const words = el('div');
            words.append(el('span', shortLabel(note) + (note.selection.url !== location.href && !note.markup ? ' · other page' : ''), { class: 'what' }),
                el('span', note.comment.trim() || 'No request yet', { class: note.comment.trim() ? 'said' : 'said missing' }));
            row.append(el('span', String(index + 1), { class: 'count' }), words);
            item.append(row, button('×', 'Delete note ' + (index + 1), () => post('remove', { id: note.id }), 'btn ghost danger'));
            list.append(item);
        }
        const footer = el('footer');
        footer.append(button('Copy all', 'Copy every note with its page context', () => { flushEdit(); post('copy'); }, 'btn ghost'),
            button('Clear', 'Delete every note', () => { flushEdit(); post('clear'); }, 'btn ghost danger'),
            el('span', undefined, { class: 'spacer' }), el('span', sendKeys + ' sends', { class: 'note' }));
        tray.append(header, list, footer);
        return tray;
    }
    function undo() {
        if (drawing?.shapes.length) { drawing.redo.push(drawing.shapes.pop()); redraw(); }
    }
    function redo() {
        if (drawing?.redo.length) { drawing.shapes.push(drawing.redo.pop()); redraw(); }
    }
    function redraw() {
        if (!drawing?.canvas) return;
        const { canvas, image, shapes, current: live } = drawing, ctx = canvas.getContext('2d');
        ctx.clearRect(0, 0, canvas.width, canvas.height); ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
        for (const shape of [...shapes, ...(live ? [live] : [])]) {
            const a = shape.points[0], b = shape.points.at(-1); if (!a || !b) continue;
            ctx.save();
            ctx.strokeStyle = ctx.fillStyle = shape.color; ctx.lineWidth = shape.width; ctx.lineCap = ctx.lineJoin = 'round';
            if (shape.tool === 'highlight') { ctx.globalAlpha = 0.35; ctx.lineWidth = shape.width * 5; }
            if (shape.tool === 'text') {
                ctx.font = '600 ' + (16 + shape.width * 3) + 'px system-ui,sans-serif';
                ctx.lineWidth = 4; ctx.strokeStyle = 'rgba(0,0,0,.55)'; ctx.strokeText(shape.text, a.x, a.y); ctx.fillText(shape.text, a.x, a.y);
                ctx.restore(); continue;
            }
            ctx.beginPath();
            if (shape.tool === 'rect') ctx.rect(a.x, a.y, b.x - a.x, b.y - a.y);
            else { ctx.moveTo(a.x, a.y); for (const p of shape.points.slice(1)) ctx.lineTo(p.x, p.y); }
            ctx.stroke();
            if (shape.tool === 'arrow') {
                const angle = Math.atan2(b.y - a.y, b.x - a.x), size = 12 + shape.width * 2;
                ctx.beginPath(); ctx.moveTo(b.x, b.y);
                ctx.lineTo(b.x - size * Math.cos(angle - .5), b.y - size * Math.sin(angle - .5));
                ctx.moveTo(b.x, b.y); ctx.lineTo(b.x - size * Math.cos(angle + .5), b.y - size * Math.sin(angle + .5)); ctx.stroke();
            }
            ctx.restore();
        }
        const save = root.querySelector('[data-design-save]');
        if (save) save.disabled = !shapes.length;
    }
    function renderDrawing(ui) {
        const layer = el('div', undefined, { class: 'drawing', role: 'dialog', 'aria-label': 'Mark up a screenshot' });
        const canvas = el('canvas'), scale = Math.min(1, 2048 / Math.max(drawing.image.width, drawing.image.height));
        canvas.width = Math.round(drawing.image.width * scale); canvas.height = Math.round(drawing.image.height * scale);
        drawing.canvas = canvas;
        const tools = el('div', undefined, { class: 'panel tools', role: 'toolbar', 'aria-label': 'Drawing tools' });
        const tool = (name, label, title) => {
            const b = button(label, title, () => { drawing.tool = name; render(); });
            b.setAttribute('aria-pressed', String(drawing.tool === name));
            return b;
        };
        tools.append(tool('arrow', '↗ Arrow', 'Arrow'), tool('rect', '▢ Box', 'Rectangle'), tool('pen', '✎ Pen', 'Freehand'),
            tool('highlight', '▬ Highlight', 'Highlighter'), tool('text', 'T Text', 'Text: type below, then click where it goes'),
            el('span', undefined, { class: 'sep' }));
        for (const [color, name] of [['#ef4444', 'Red'], ['#f59e0b', 'Amber'], ['#22c55e', 'Green'], ['#3b82f6', 'Blue'], ['#a855f7', 'Purple'], ['#ffffff', 'White']]) {
            const swatch = button('', name, () => { drawing.color = color; render(); }, 'swatch');
            swatch.style.background = color; swatch.setAttribute('aria-pressed', String(drawing.color === color));
            tools.append(swatch);
        }
        const width = el('select', undefined, { class: 'small', 'aria-label': 'Stroke width' });
        for (const [value, name] of [[2, 'Thin'], [4, 'Medium'], [8, 'Thick']]) width.append(el('option', name, { value }));
        width.value = drawing.width;
        width.addEventListener('change', () => { drawing.width = Number(width.value); });
        tools.append(width);
        if (drawing.tool === 'text') {
            const words = el('input', undefined, { type: 'text', placeholder: 'Label text', maxlength: '160', 'aria-label': 'Label text', 'data-keep': 'words' });
            words.value = drawing.words;
            words.addEventListener('input', () => { drawing.words = words.value; });
            tools.append(words);
        }
        const saveNote = button('Add note', 'Attach this marked-up screenshot as a note', () => {
            if (!drawing.shapes.length) return;
            const image = canvas.toDataURL('image/png'), selection = drawing.selection;
            if (image.length > 11 * 1024 * 1024) { say('This capture is too large. Try a smaller window.'); return; }
            post('markup', { image, selection }); drawing = null; awaiting = true; mode = 'pick'; render();
        }, 'btn primary');
        saveNote.dataset.designSave = ''; saveNote.disabled = !drawing.shapes.length;
        tools.append(el('span', undefined, { class: 'sep' }), button('↶', 'Undo · ' + (mac ? '⌘Z' : 'Ctrl+Z'), undo, 'btn ghost'),
            button('↷', 'Redo', redo, 'btn ghost'), button('Cancel', 'Discard this markup · Esc', () => { drawing = null; mode = 'pick'; render(); }, 'btn ghost'), saveNote);
        const point = event => { const r = canvas.getBoundingClientRect(); return { x: (event.clientX - r.left) * canvas.width / r.width, y: (event.clientY - r.top) * canvas.height / r.height }; };
        canvas.addEventListener('pointerdown', event => {
            if (event.button !== 0 || drawing.shapes.length >= 100) return;
            if (drawing.tool === 'text' && !drawing.words.trim()) { say('Type the label first, then click where it goes.'); return; }
            event.preventDefault(); canvas.setPointerCapture(event.pointerId); drawing.redo = [];
            drawing.current = { tool: drawing.tool, color: drawing.color, width: drawing.width, text: drawing.words, points: [point(event)] };
            if (drawing.tool === 'text') { drawing.shapes.push(drawing.current); drawing.current = null; redraw(); }
        });
        canvas.addEventListener('pointermove', event => {
            if (!drawing.current) return;
            const free = drawing.current.tool === 'pen' || drawing.current.tool === 'highlight';
            if (free && drawing.current.points.length < 2000) drawing.current.points.push(point(event));
            else drawing.current.points[1] = point(event);
            redraw();
        });
        const finish = () => { if (drawing.current) { drawing.shapes.push(drawing.current); drawing.current = null; redraw(); } };
        canvas.addEventListener('pointerup', finish); canvas.addEventListener('pointercancel', finish);
        const stage = el('div', undefined, { class: 'stage' });
        stage.append(canvas);
        layer.append(tools, stage);
        ui.append(layer);
        if (toast) ui.append(el('div', toast, { class: 'panel pill', role: 'status' }));
        redraw();
    }
    const api = {
        configure(next) {
            const oldToken = config?.token;
            if (oldToken === next.token && pendingEdit) {
                const draft = pendingEdit, note = next.notes.find(n => n.id === draft.id);
                if (note) { note.comment = draft.comment; note.intent = draft.intent; }
                flushEdit();
            }
            if (oldToken && oldToken !== next.token && host) unmount();
            config = { ...next };
            if (!config.enabled) { if (host) unmount(); return; }
            const fresh = !host;
            if (fresh) { mount(); mode = 'pick'; known = new Set(config.notes.map(n => n.id)); }
            const palette = config.palette || {};
            for (const key of ['bg', 'raised', 'fg', 'muted', 'line', 'accent', 'accent_fg', 'danger', 'warning']) {
                if (palette[key]) host.style.setProperty('--' + key.replace('_', '-'), palette[key]);
            }
            root.querySelector('.dd').style.colorScheme = config.dark ? 'dark' : 'light';
            const added = config.notes.filter(n => !known.has(n.id));
            known = new Set(config.notes.map(n => n.id));
            let focus = false;
            if (awaiting && added.length) {
                clearTimeout(captureTimer);
                awaiting = false; card = added.at(-1).id; cardIsNew = true; cardError = ''; mode = 'card'; trayOpen = false; focus = true;
            }
            if (card && !current()) { card = null; cardIsNew = false; if (mode === 'card') mode = 'pick'; }
            if (mode !== 'busy') host.style.setProperty('visibility', 'visible', 'important');
            render();
            if (focus) root.querySelector('.card textarea')?.focus({ preventScroll: true });
            if (fresh) post('ready');
        },
        // Errors and confirmations from the app.
        error(message) {
            clearTimeout(captureTimer);
            if (mode === 'busy') { mode = 'pick'; awaiting = false; }
            host?.style.setProperty('visibility', 'visible', 'important');
            say(message);
        },
        captured(data, selection) {
            clearTimeout(captureTimer);
            if (!host) return;
            awaiting = false;
            const image = new Image();
            image.onload = () => {
                if (!host) return;
                drawing = { image, selection, shapes: [], redo: [], current: null, tool: 'arrow', color: '#ef4444', width: 4, words: '' };
                mode = 'draw';
                host.style.setProperty('visibility', 'visible', 'important'); render();
            };
            image.onerror = () => api.error('Could not open the page capture.'); image.src = data;
        },
        tool(name) { if (name === 'draw') draw(); },
        open(id) { openNote(id); },
        flush(request) { flushEdit(); post('flushed', { request }); },
        destroy: unmount,
    };
    Object.defineProperty(window, '__dragentDesign', { value: api, configurable: true });
})();
