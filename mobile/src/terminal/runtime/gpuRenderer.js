/**
 * The terminal's WebGL2 renderer. Runs only inside the isolated terminal
 * document, next to browserRuntime.js, which decides when it draws; the 2D
 * canvas renderer there takes over whenever this one cannot.
 *
 * Every glyph is drawn once into an atlas texture; a frame is instanced
 * quads: cell backgrounds, then glyphs, then decorations, then the cursor.
 * Rows keep their instance data in the slot of the row ring they occupy, so
 * scrolling only changes a few uniforms and new output rewrites only the rows
 * it changed. Nothing here touches the network.
 */
export function gpuRenderer(kit, gk) {
  const WORDS = gk.WORDS, STRIDE = WORDS * 4;
  const ATLAS_START = 1024, ATLAS_LIMIT = 4096;
  /** A glyph box never grows wider than this many cells; ink past it is cut off. */
  const MAX_GLYPH_CELLS = 6;
  const FULL = { atlasFull: true };

  const VERTEX = `#version 300 es
layout(location = 0) in vec2 a_corner;
layout(location = 1) in vec4 a_geom;
layout(location = 2) in float a_extra;
layout(location = 3) in uint a_color;
uniform vec2 u_view;
uniform int u_cols;
uniform float u_ring, u_first, u_visible, u_top, u_sub, u_rowPx;
uniform bool u_absolute;
out vec2 v_local;
flat out vec4 v_geom;
flat out float v_extra;
flat out vec4 v_color;
// Which row this instance's slot shows, or false when the slot is not on screen.
bool rowTop(out float top) {
  if (u_absolute) { top = 0.0; return true; }
  float rel = mod(float(gl_InstanceID / u_cols) - u_first + u_ring, u_ring);
  top = u_top + rel * u_rowPx - u_sub;
  return rel < u_visible;
}
vec4 unpackColor(uint c) {
  return vec4(float(c >> 24u), float((c >> 16u) & 255u), float((c >> 8u) & 255u), float(c & 255u)) / 255.0;
}
void place(vec2 local, float top) {
  vec2 px = vec2(local.x, top + local.y);
  gl_Position = vec4(px.x / u_view.x * 2.0 - 1.0, 1.0 - px.y / u_view.y * 2.0, 0.0, 1.0);
}
`;
  const RECT_VERTEX = `${VERTEX}
void main() {
  float top;
  if (!rowTop(top) || a_geom.y <= a_geom.x || a_geom.w <= a_geom.z) { gl_Position = vec4(-2.0, -2.0, 0.0, 1.0); return; }
  vec2 lo = floor(a_geom.xz), hi = ceil(a_geom.yw);
  v_local = mix(lo, hi, a_corner);
  v_geom = a_geom; v_extra = a_extra; v_color = unpackColor(a_color);
  place(v_local, top);
}`;
  const GLYPH_VERTEX = `${VERTEX}
void main() {
  float top;
  if (!rowTop(top) || a_geom.y <= 0.0) { gl_Position = vec4(-2.0, -2.0, 0.0, 1.0); return; }
  vec2 size = vec2(a_geom.y, u_rowPx);
  vec2 local = vec2(a_geom.x, 0.0) + a_corner * size;
  // v_local is the texel within the atlas.
  v_local = a_geom.zw + a_corner * size;
  v_geom = a_geom; v_extra = a_extra; v_color = unpackColor(a_color);
  place(local, top);
}`;
  // Coverage is the exact share of the pixel inside the box, which is how the
  // 2D canvas antialiases a rectangle with fractional edges.
  const RECT_FRAGMENT = `#version 300 es
precision highp float;
in vec2 v_local;
flat in vec4 v_geom;
flat in float v_extra;
flat in vec4 v_color;
uniform vec4 u_bands;
uniform float u_rule;
out vec4 color;
float overlap(float from, float a, float b) { return clamp(min(from + 1.0, b) - max(from, a), 0.0, 1.0); }
void main() {
  vec2 p = floor(v_local);
  float cx = overlap(p.x, v_geom.x, v_geom.y), cy;
  int bands = int(v_extra + 0.5);
  if (bands == 0) cy = overlap(p.y, v_geom.z, v_geom.w);
  else {
    cy = 0.0;
    if ((bands & 1) != 0) cy += overlap(p.y, u_bands.x, u_bands.x + u_rule);
    if ((bands & 2) != 0) cy += overlap(p.y, u_bands.y, u_bands.y + u_rule);
    if ((bands & 4) != 0) cy += overlap(p.y, u_bands.z, u_bands.z + u_rule);
    if ((bands & 8) != 0) cy += overlap(p.y, u_bands.w, u_bands.w + u_rule);
    cy = min(cy, 1.0);
  }
  float a = v_color.a * cx * cy;
  if (a <= 0.0) discard;
  color = vec4(v_color.rgb * a, a);
}`;
  const GLYPH_FRAGMENT = `#version 300 es
precision highp float;
in vec2 v_local;
flat in vec4 v_geom;
flat in float v_extra;
flat in vec4 v_color;
uniform sampler2D u_atlas;
out vec4 color;
void main() {
  vec4 texel = texelFetch(u_atlas, ivec2(floor(v_local)), 0);
  if (texel.a <= 0.0) discard;
  if (v_extra > 0.5) color = vec4(texel.rgb * texel.a, texel.a) * v_color.a;
  else { float a = texel.a * v_color.a; color = vec4(v_color.rgb * a, a); }
}`;

  /**
   * Make a renderer drawing on `canvas`, or null when the page has no
   * WebGL2. `hooks.lost()` runs when the context goes away and
   * `hooks.restored()` once it is back with fresh resources.
   */
  return (canvas, hooks) => {
    const gl = canvas.getContext('webgl2', { alpha: true, premultipliedAlpha: true, antialias: false, depth: false, stencil: false, preserveDrawingBuffer: false, powerPreference: 'low-power' });
    if (!gl || typeof gl.createVertexArray !== 'function' || typeof gl.drawArraysInstanced !== 'function') return null;
    let lost = false, res = null;
    // Instance data mirrors, one region of `cols` instances per ring slot.
    let ring = null, cols = 0, arrays = null, dirty = new Set();
    // Atlas state: glyph boxes by family (bold/italic and phase) then text.
    let families = [], packer = null, atlasKey = '', atlasW = 0, atlasH = 0, scratch = null, scratchCanvas = null;
    let geometry = null;
    const colors = new Map();
    const box = { x: 0, w: 0, ax: 0, ay: 0, kind: 0 };

    const compile = (vertex, fragment) => {
      const program = gl.createProgram();
      for (const [type, source] of [[gl.VERTEX_SHADER, vertex], [gl.FRAGMENT_SHADER, fragment]]) {
        const shader = gl.createShader(type);
        gl.shaderSource(shader, source); gl.compileShader(shader);
        if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw new Error(`shader: ${gl.getShaderInfoLog(shader)}`);
        gl.attachShader(program, shader);
        gl.deleteShader(shader);
      }
      gl.linkProgram(program);
      if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(`program: ${gl.getProgramInfoLog(program)}`);
      const uniforms = {};
      const count = gl.getProgramParameter(program, gl.ACTIVE_UNIFORMS);
      for (let index = 0; index < count; index++) {
        const name = gl.getActiveUniform(program, index).name;
        uniforms[name] = gl.getUniformLocation(program, name);
      }
      return { program, uniforms };
    };
    /** A vertex array drawing the shared quad once per instance in `buffer`. */
    const instanced = (quad, buffer) => {
      const vao = gl.createVertexArray();
      gl.bindVertexArray(vao);
      gl.bindBuffer(gl.ARRAY_BUFFER, quad);
      gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
      gl.enableVertexAttribArray(1); gl.vertexAttribPointer(1, 4, gl.FLOAT, false, STRIDE, 0); gl.vertexAttribDivisor(1, 1);
      gl.enableVertexAttribArray(2); gl.vertexAttribPointer(2, 1, gl.FLOAT, false, STRIDE, 16); gl.vertexAttribDivisor(2, 1);
      gl.enableVertexAttribArray(3); gl.vertexAttribIPointer(3, 1, gl.UNSIGNED_INT, STRIDE, 20); gl.vertexAttribDivisor(3, 1);
      gl.bindVertexArray(null);
      return vao;
    };
    const makeTexture = (width, height) => {
      const texture = gl.createTexture();
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, width, height);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      return texture;
    };
    /** Programs, buffers and an empty atlas: made at start and again after a context comes back. */
    const setup = () => {
      const rect = compile(RECT_VERTEX, RECT_FRAGMENT), glyph = compile(GLYPH_VERTEX, GLYPH_FRAGMENT);
      const quad = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, quad);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]), gl.STATIC_DRAW);
      const buffers = { bg: gl.createBuffer(), glyphs: gl.createBuffer(), deco: gl.createBuffer(), cursor: gl.createBuffer() };
      const vaos = {};
      for (const name of Object.keys(buffers)) vaos[name] = instanced(quad, buffers[name]);
      gl.bindBuffer(gl.ARRAY_BUFFER, buffers.cursor);
      gl.bufferData(gl.ARRAY_BUFFER, 4 * STRIDE, gl.DYNAMIC_DRAW);
      const max = Math.min(ATLAS_LIMIT, gl.getParameter(gl.MAX_TEXTURE_SIZE) || ATLAS_START);
      atlasW = Math.min(ATLAS_START, max); atlasH = atlasW;
      res = { rect, glyph, quad, buffers, vaos, max, atlas: makeTexture(atlasW, atlasH), cursor: makeArrays(4), capacity: 0 };
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
      gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
      packer = gk.createPacker(atlasW, atlasH);
      families = [];
      atlasKey = '';
      ring = null;
    };
    /** A small 2D canvas glyphs are drawn on before they go into the atlas. */
    const scratchContext = () => {
      if (!scratch) {
        scratchCanvas = document.createElement('canvas');
        scratch = scratchCanvas.getContext('2d', { willReadFrequently: true });
      }
      return scratch;
    };
    const makeArrays = (instances) => {
      const buffer = new ArrayBuffer(instances * STRIDE);
      return { f32: new Float32Array(buffer), u32: new Uint32Array(buffer) };
    };

    // ── Atlas ────────────────────────────────────────────────────────────────
    const resetAtlas = () => {
      packer.reset();
      families = [];
      if (ring) for (const slot of ring.slots) slot.gpu = false;
    };
    /** Grow the atlas texture, keeping what is in it; false at the size limit. */
    const growAtlas = () => {
      const next = gk.nextAtlasSize(atlasW, atlasH, res.max);
      if (!next) return false;
      const texture = makeTexture(next.width, next.height);
      const framebuffer = gl.createFramebuffer();
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, framebuffer);
      gl.framebufferTexture2D(gl.READ_FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, res.atlas, 0);
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.copyTexSubImage2D(gl.TEXTURE_2D, 0, 0, 0, 0, 0, atlasW, atlasH);
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, null);
      gl.deleteFramebuffer(framebuffer);
      gl.deleteTexture(res.atlas);
      res.atlas = texture; atlasW = next.width; atlasH = next.height;
      packer.grow(atlasW, atlasH);
      return true;
    };
    /**
     * Draw one glyph into the atlas the way the 2D renderer draws it on a row:
     * same font, transform and baseline, at its quarter-pixel phase. Returns
     * its box, or null when it has no ink.
     */
    const rasterise = (text, flags, cells, phase) => {
      const { dpr, size, scale, rowPx, cellPx } = geometry;
      const g = scratchContext(), fontName = kit.font(flags, size);
      if (g.font !== fontName) g.font = fontName;
      g.setTransform(1, 0, 0, 1, 0, 0);
      const metrics = g.measureText(text);
      const offset = phase / gk.PHASES;
      const left = Math.max(0, Math.ceil((metrics.actualBoundingBoxLeft || 0) * dpr)) + 1;
      const ink = Math.max(cells * cellPx, (metrics.actualBoundingBoxRight || 0) * dpr + offset, metrics.width * dpr);
      const width = Math.min(left + Math.ceil(ink) + 1, Math.ceil(MAX_GLYPH_CELLS * cellPx) + left + 1);
      if (scratchCanvas.width < width || scratchCanvas.height < rowPx) {
        // Resizing a canvas resets its drawing state, font included.
        scratchCanvas.width = Math.max(scratchCanvas.width, width); scratchCanvas.height = Math.max(scratchCanvas.height, rowPx);
        g.font = fontName;
      }
      g.clearRect(0, 0, width, rowPx);
      g.setTransform(dpr, 0, 0, dpr, left + offset, 0);
      g.textBaseline = 'top';
      g.fillStyle = '#ffffff';
      g.globalAlpha = 1;
      g.fillText(text, 0, 2 * scale);
      g.setTransform(1, 0, 0, 1, 0, 0);
      const image = g.getImageData(0, 0, width, rowPx), data = image.data;
      let inked = false, colour = false;
      for (let index = 3; index < data.length; index += 4) {
        const alpha = data[index];
        if (alpha === 0) continue;
        inked = true;
        // Text drawn in white stays white; a colour glyph keeps its own colours.
        if (alpha >= 64 && Math.min(data[index - 3], data[index - 2], data[index - 1]) < 200) { colour = true; break; }
      }
      if (!inked) return null;
      let at = packer.alloc(width, rowPx);
      while (!at && growAtlas()) at = packer.alloc(width, rowPx);
      if (!at) throw FULL;
      gl.bindTexture(gl.TEXTURE_2D, res.atlas);
      gl.texSubImage2D(gl.TEXTURE_2D, 0, at.x, at.y, width, rowPx, gl.RGBA, gl.UNSIGNED_BYTE, data);
      return { ax: at.x, ay: at.y, w: width, left, kind: colour ? gk.COLOUR : gk.MASK };
    };
    const glyphAt = (text, flags, col, cells) => {
      const { whole, phase } = gk.placement(col, geometry.cellPx);
      const index = gk.family(flags, phase);
      const glyphs = families[index] ||= new Map();
      const key = cells === 1 ? text : `\u0001${cells}${text}`;
      let entry = glyphs.get(key);
      if (entry === undefined) { entry = rasterise(text, flags, cells, phase); glyphs.set(key, entry); }
      if (entry === null) return null;
      box.x = whole - entry.left; box.w = entry.w; box.ax = entry.ax; box.ay = entry.ay; box.kind = entry.kind;
      return box;
    };
    const colorOf = (css) => {
      let packed = colors.get(css);
      if (packed === undefined) {
        packed = gk.parseColor(css);
        if (packed === null) {
          // Anything else, the browser normalises to a form the parser knows.
          const g = scratchContext();
          g.fillStyle = '#000000'; g.fillStyle = css;
          packed = gk.parseColor(g.fillStyle) ?? 0x000000ff;
        }
        colors.set(css, packed);
      }
      return packed;
    };

    // ── Rows ─────────────────────────────────────────────────────────────────
    const useRing = (next, nextCols) => {
      if (ring === next && cols === nextCols) return;
      ring = next; cols = nextCols;
      const instances = ring.count * cols;
      arrays = { bg: makeArrays(instances), glyphs: makeArrays(instances), deco: makeArrays(instances) };
      for (const name of ['bg', 'glyphs', 'deco']) {
        gl.bindBuffer(gl.ARRAY_BUFFER, res.buffers[name]);
        gl.bufferData(gl.ARRAY_BUFFER, arrays[name].f32, gl.DYNAMIC_DRAW);
      }
      res.capacity = instances;
      for (const slot of ring.slots) slot.gpu = false;
      dirty.clear();
    };
    const buildSlot = (index) => {
      const slot = ring.slots[index];
      gk.writeRow(slot.runs, geometry, index * cols, arrays, glyphAt, colorOf);
      slot.gpu = true;
      dirty.add(index);
    };
    /** Send every rewritten slot to its buffers, one upload per stretch of neighbouring slots. */
    const upload = () => {
      if (dirty.size === 0) return;
      const order = [...dirty].sort((a, b) => a - b);
      dirty.clear();
      for (let start = 0; start < order.length;) {
        let end = start;
        while (end + 1 < order.length && order[end + 1] === order[end] + 1) end++;
        const from = order[start] * cols * WORDS, length = (order[end] - order[start] + 1) * cols * WORDS;
        for (const name of ['bg', 'glyphs', 'deco']) {
          gl.bindBuffer(gl.ARRAY_BUFFER, res.buffers[name]);
          gl.bufferSubData(gl.ARRAY_BUFFER, from * 4, arrays[name].f32, from, length);
        }
        start = end + 1;
      }
    };

    // ── Frames ───────────────────────────────────────────────────────────────
    const clearTo = (css) => {
      const [r, g, b, a] = gk.unpack(colorOf(css));
      gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.disable(gl.SCISSOR_TEST);
      gl.clearColor(r * a, g * a, b * a, a);
      gl.clear(gl.COLOR_BUFFER_BIT);
    };
    const common = (program, frame, absolute) => {
      const u = program.uniforms;
      gl.useProgram(program.program);
      gl.uniform2f(u.u_view, canvas.width, canvas.height);
      gl.uniform1i(u.u_cols, Math.max(1, cols));
      gl.uniform1f(u.u_ring, ring ? ring.count : 1);
      gl.uniform1f(u.u_first, frame.firstSlot);
      gl.uniform1f(u.u_visible, frame.visible);
      gl.uniform1f(u.u_top, frame.topPx);
      gl.uniform1f(u.u_sub, frame.sub);
      gl.uniform1f(u.u_rowPx, geometry.rowPx);
      gl.uniform1i(u.u_absolute, absolute ? 1 : 0);
    };
    /**
     * Draw a frame. `frame` says which lines show: `firstSlot` is the ring
     * slot of the top line, `visible` how many lines (slots) follow it, `sub`
     * how many device pixels of the top line are scrolled off, and `rows` the
     * grid's height in rows; `cursor` is the cursor's cell or null.
     */
    const draw = (frame) => {
      useRing(frame.ring, frame.cols);
      // A full atlas is emptied once and the screen's rows rebuilt; if they
      // still do not fit, the 2D renderer takes over (the caller catches).
      for (let attempt = 0; ; attempt++) {
        try {
          for (let index = 0; index < frame.visible; index++) {
            const slotIndex = (frame.firstSlot + index) % ring.count;
            if (!ring.slots[slotIndex].gpu) buildSlot(slotIndex);
          }
          break;
        } catch (error) {
          if (error !== FULL || attempt > 0) throw error === FULL ? new Error('glyph atlas full') : error;
          resetAtlas();
        }
      }
      upload();
      clearTo(frame.background);
      const { rowPx } = geometry, height = canvas.height;
      gl.enable(gl.SCISSOR_TEST);
      gl.scissor(0, height - (frame.topPx + frame.rows * rowPx), canvas.width, frame.rows * rowPx);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
      const bands = geometry.bands;
      common(res.rect, frame, false);
      gl.uniform4f(res.rect.uniforms.u_bands, bands.underline, bands.double, bands.strike, bands.overline);
      gl.uniform1f(res.rect.uniforms.u_rule, bands.rule);
      gl.bindVertexArray(res.vaos.bg);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, res.capacity);
      common(res.glyph, frame, false);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, res.atlas);
      gl.uniform1i(res.glyph.uniforms.u_atlas, 0);
      gl.bindVertexArray(res.vaos.glyphs);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, res.capacity);
      common(res.rect, frame, false);
      gl.bindVertexArray(res.vaos.deco);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, res.capacity);
      if (frame.cursor) {
        gl.disable(gl.SCISSOR_TEST);
        const c = frame.cursor, rects = gk.cursorRects(c.col, c.row, c.cell, c.line, c.top, c.scale, geometry.dpr), color = colorOf(c.color);
        rects.forEach(([x0, x1, y0, y1], index) => {
          const o = index * WORDS, cursor = res.cursor;
          cursor.f32[o] = x0; cursor.f32[o + 1] = x1; cursor.f32[o + 2] = y0; cursor.f32[o + 3] = y1; cursor.f32[o + 4] = 0; cursor.u32[o + 5] = color;
        });
        gl.bindBuffer(gl.ARRAY_BUFFER, res.buffers.cursor);
        gl.bufferSubData(gl.ARRAY_BUFFER, 0, res.cursor.f32);
        common(res.rect, frame, true);
        gl.bindVertexArray(res.vaos.cursor);
        gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, 4);
      }
      gl.bindVertexArray(null);
      gl.disable(gl.BLEND);
      gl.disable(gl.SCISSOR_TEST);
    };

    canvas.addEventListener('webglcontextlost', (event) => {
      // Asking for the context back; until then the 2D renderer draws.
      event.preventDefault();
      lost = true; res = null; ring = null;
      hooks.lost();
    });
    canvas.addEventListener('webglcontextrestored', () => {
      try { setup(); lost = false; colors.clear(); hooks.restored(); } catch { hooks.failed(); }
    });
    setup();
    return {
      get lost() { return lost || gl.isContextLost(); },
      /**
       * Size the drawing buffer and, given `next`, say how glyphs are laid
       * out; a new font size or scale empties the atlas.
       */
      layout(widthPx, heightPx, next) {
        if (canvas.width !== widthPx) canvas.width = widthPx;
        if (canvas.height !== heightPx) canvas.height = heightPx;
        if (!next) return;
        const key = `${next.size}:${next.scale}:${next.dpr}:${next.rowPx}:${next.cellPx}`;
        geometry = { ...next, bands: gk.bandGeometry(next.line, next.scale, next.dpr) };
        if (key !== atlasKey) { atlasKey = key; resetAtlas(); }
      },
      clear: (css) => clearTo(css),
      draw,
      /** For tests and the bench: how much of the atlas is in use. */
      stats: () => ({ atlas: { width: atlasW, height: atlasH, used: packer ? packer.used : 0 }, glyphs: families.reduce((sum, map) => sum + (map ? map.size : 0), 0) }),
      /** Give the GPU memory back now. */
      dispose() {
        if (!res || lost) return;
        for (const name of Object.keys(res.buffers)) gl.deleteBuffer(res.buffers[name]);
        gl.deleteBuffer(res.quad); gl.deleteTexture(res.atlas);
        gl.deleteProgram(res.rect.program); gl.deleteProgram(res.glyph.program);
        gl.getExtension('WEBGL_lose_context')?.loseContext();
        res = null;
      },
    };
  };
}
