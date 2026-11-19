#!/usr/bin/env python3
"""Generate the macOS DMG 'drag to Applications' background image.

Run from the repo root:  python3 scripts/macos/gen-dmg-background.py
(Needs Pillow; use a venv if your Python is externally managed:
   python3 -m venv .venv && .venv/bin/pip install pillow
   .venv/bin/python scripts/macos/gen-dmg-background.py )

Paints the same gradient/dot/glow branding as the Windows installer art, with an
arrow from the app-icon slot to the Applications-folder slot. package-macos.sh
lays the real .app and /Applications alias on top of the two slots.

Window/background is 660x400 logical points; we render at @2x (1320x800) and also
emit a 1x PNG so Finder can show a crisp background on Retina.
"""
import colorsys
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ICON = "app/assets/dragent/icons/dragent-icon.png"
OUT = "app/assets/resources/mac/dmg-background"

# Brand accent (Return Fold cyan). The auto-derivation below tends to land on
# the mark's shadow-face blue; set to None to fall back to accent_from_icon().
BRAND_ACCENT = (25, 170, 216)

W, H = 660, 400
# Icon slot centers (must match the Finder icon positions in package-macos.sh).
APP_XY = (170, 205)
APPS_XY = (490, 205)


def clamp(v):
    return max(0, min(255, int(round(v))))


def lerp(a, b, t):
    return tuple(clamp(a[i] + (b[i] - a[i]) * t) for i in range(3))


def shade(rgb, ss=None, vv=None):
    h, s, v = colorsys.rgb_to_hsv(*[c / 255 for c in rgb])
    if ss is not None:
        s = ss
    if vv is not None:
        v = vv
    r, g, b = colorsys.hsv_to_rgb(h, min(1, s), min(1, v))
    return (clamp(r * 255), clamp(g * 255), clamp(b * 255))


def accent_from_icon(icon):
    small = icon.convert("RGBA").resize((80, 80))
    px = small.load()
    best, best_score, counts = (90, 110, 245), -1.0, {}
    for y in range(80):
        for x in range(80):
            r, g, b, a = px[x, y]
            if a < 200:
                continue
            key = (r // 16, g // 16, b // 16)
            cr, cg, cb, cn = counts.get(key, (0, 0, 0, 0))
            counts[key] = (cr + r, cg + g, cb + b, cn + 1)
    for (r, g, b, n) in counts.values():
        if n == 0:
            continue
        r, g, b = r / n, g / n, b / n
        _, s, v = colorsys.rgb_to_hsv(r / 255, g / 255, b / 255)
        score = (s ** 1.5) * (0.35 + 0.65 * v) * (n ** 0.25)
        if score > best_score:
            best_score, best = score, (r, g, b)
    return tuple(clamp(c) for c in best)


def load_font(size):
    for path in ("/System/Library/Fonts/SFNS.ttf",
                 "/System/Library/Fonts/Helvetica.ttc",
                 "/Library/Fonts/Arial.ttf"):
        try:
            return ImageFont.truetype(path, size)
        except Exception:
            continue
    return ImageFont.load_default()


def slots_overlay(size_wh, slots, size, accent, scale):
    """Subtle translucent framed plates behind the icon slots.

    Drawn on a transparent layer so the low alphas actually blend (ImageDraw
    replaces pixels when drawing directly onto an opaque image, ignoring alpha)."""
    layer = Image.new("RGBA", size_wh, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    half = size // 2 + 18 * scale
    for (cx, cy) in slots:
        box = [cx - half, cy - half, cx + half, cy + half]
        d.rounded_rectangle(box, radius=24 * scale,
                            fill=shade(accent, ss=0.45, vv=1.0) + (26,),
                            outline=shade(accent, ss=0.3, vv=1.0) + (70,),
                            width=max(1, 2 * scale))
    return layer


def render(scale):
    w, h = W * scale, H * scale
    icon = Image.open(ICON)
    accent = BRAND_ACCENT or accent_from_icon(icon)

    # gradient
    img = Image.new("RGB", (w, h))
    px = img.load()
    top = shade(accent, ss=0.72, vv=0.55)
    bottom = shade(accent, ss=0.88, vv=0.13)
    for y in range(h):
        for x in range(w):
            t = (x / max(1, w - 1) * 0.35 + y / max(1, h - 1) * 0.65)
            px[x, y] = lerp(top, bottom, t)
    img = img.convert("RGBA")

    # dot texture
    dot = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    dd = ImageDraw.Draw(dot)
    light = shade(accent, ss=0.4, vv=0.95)
    step = 18 * scale
    for y in range(step // 2, h, step):
        for x in range(step // 2, w, step):
            r = 1 * scale
            dd.ellipse([x - r, y - r, x + r, y + r], fill=light + (22,))
    img = Image.alpha_composite(img, dot)

    # soft glow behind the app slot
    glow = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    gr = 150 * scale
    gcx, gcy = APP_XY[0] * scale, APP_XY[1] * scale
    gd.ellipse([gcx - gr, gcy - gr, gcx + gr, gcy + gr],
               fill=shade(accent, ss=0.55, vv=1.0) + (90,))
    glow = glow.filter(ImageFilter.GaussianBlur(gr * 0.5))
    img = Image.alpha_composite(img, glow)

    # icon slots (subtle translucent plates the real icons sit on)
    slots = [(APP_XY[0] * scale, APP_XY[1] * scale),
             (APPS_XY[0] * scale, APPS_XY[1] * scale)]
    img = Image.alpha_composite(img, slots_overlay((w, h), slots, 112 * scale, accent, scale))

    d = ImageDraw.Draw(img)

    # arrow from app slot -> applications slot
    ay = APP_XY[1] * scale
    x0 = (APP_XY[0] + 92) * scale
    x1 = (APPS_XY[0] - 92) * scale
    col = (255, 255, 255, 235)
    d.line([(x0, ay), (x1 - 10 * scale, ay)], fill=col, width=6 * scale)
    d.polygon([(x1, ay), (x1 - 20 * scale, ay - 13 * scale),
               (x1 - 20 * scale, ay + 13 * scale)], fill=col)

    # heading
    title_f = load_font(30 * scale)
    tt = "Install Dragent"
    tw = d.textbbox((0, 0), tt, font=title_f)[2]
    d.text(((w - tw) / 2, 40 * scale), tt, font=title_f, fill=(255, 255, 255))
    sub_f = load_font(15 * scale)
    st = "Drag the app onto the Applications folder"
    sw = d.textbbox((0, 0), st, font=sub_f)[2]
    d.text(((w - sw) / 2, 84 * scale), st, font=sub_f, fill=shade(accent, ss=0.22, vv=1.0))

    return img.convert("RGB")


def main():
    render(1).save(f"{OUT}.png")       # 660x400
    render(2).save(f"{OUT}@2x.png")    # 1320x800 (Retina)
    for suffix in ("", "@2x"):
        im = Image.open(f"{OUT}{suffix}.png")
        print(f"{OUT}{suffix}.png: {im.size}")


if __name__ == "__main__":
    main()
