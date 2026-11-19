#!/usr/bin/env python3
"""Generate Dragent's Windows-installer wizard art as clean 24-bit BMPs.

Run from the repo root:  python3 scripts/windows/gen-installer-art.py
(Needs Pillow. If your Python is externally managed, use a venv:
   python3 -m venv .venv && .venv/bin/pip install pillow
   .venv/bin/python scripts/windows/gen-installer-art.py )

Derives an accent colour from the app icon, paints a diagonal gradient with a
subtle dot pattern and a soft glow behind the icon, then composites the icon
(and the wordmark on the banner). Outputs 24-bit BMPs (Inno-Setup friendly) into
scripts/windows/installer-images/.
"""
import colorsys
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ICON = "app/assets/dragent/icons/dragent-icon.png"
OUT_DIR = "scripts/windows/installer-images"

# Brand accent (Return Fold cyan). The auto-derivation below tends to land on
# the mark's shadow-face blue; set to None to fall back to accent_from_icon().
BRAND_ACCENT = (25, 170, 216)


def clamp(v):
    return max(0, min(255, int(round(v))))


def lerp(a, b, t):
    return tuple(clamp(a[i] + (b[i] - a[i]) * t) for i in range(3))


def accent_from_icon(icon):
    """Pick a vibrant, reasonably bright dominant colour from the icon."""
    small = icon.convert("RGBA").resize((80, 80))
    px = small.load()
    best, best_score = (90, 110, 245), -1.0  # sensible fallback (indigo)
    counts = {}
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
        h, s, v = colorsys.rgb_to_hsv(r / 255, g / 255, b / 255)
        score = (s ** 1.5) * (0.35 + 0.65 * v) * (n ** 0.25)
        if score > best_score:
            best_score, best = score, (r, g, b)
    return tuple(clamp(c) for c in best)


def shade(rgb, ss=None, vv=None):
    h, s, v = colorsys.rgb_to_hsv(*[c / 255 for c in rgb])
    if ss is not None:
        s = ss
    if vv is not None:
        v = vv
    r, g, b = colorsys.hsv_to_rgb(h, min(1, s), min(1, v))
    return (clamp(r * 255), clamp(g * 255), clamp(b * 255))


def make_panel(w, h, accent, dots=True, glow_r=None):
    top = shade(accent, ss=0.72, vv=0.60)
    bottom = shade(accent, ss=0.85, vv=0.14)
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            t = (x / max(1, w - 1) * 0.45 + y / max(1, h - 1) * 0.55)
            px[x, y] = lerp(top, bottom, t)

    if dots:
        dot = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        dd = ImageDraw.Draw(dot)
        light = shade(accent, ss=0.4, vv=0.95)
        step = 16
        for y in range(step // 2, h, step):
            for x in range(step // 2, w, step):
                dd.ellipse([x - 1, y - 1, x + 1, y + 1], fill=light + (26,))
        img = Image.alpha_composite(img.convert("RGBA"), dot).convert("RGB")

    if glow_r:
        glow = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        gd = ImageDraw.Draw(glow)
        cx, cy = w // 2, int(h * 0.34)
        gd.ellipse([cx - glow_r, cy - glow_r, cx + glow_r, cy + glow_r],
                   fill=shade(accent, ss=0.55, vv=1.0) + (120,))
        glow = glow.filter(ImageFilter.GaussianBlur(glow_r * 0.55))
        img = Image.alpha_composite(img.convert("RGBA"), glow).convert("RGB")
    return img


def paste_icon(panel, icon, size, cx, cy):
    ic = icon.convert("RGBA").resize((size, size), Image.LANCZOS)
    sh = Image.new("RGBA", panel.size, (0, 0, 0, 0))
    sh.paste((0, 0, 0, 130), (cx - size // 2, cy - size // 2 + 3), ic)
    sh = sh.filter(ImageFilter.GaussianBlur(5))
    out = Image.alpha_composite(panel.convert("RGBA"), sh)
    out.paste(ic, (cx - size // 2, cy - size // 2), ic)
    return out.convert("RGB")


def load_font(size):
    for path in (
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/Library/Fonts/Arial.ttf",
        "C:/Windows/Fonts/segoeui.ttf",
    ):
        try:
            return ImageFont.truetype(path, size)
        except Exception:
            continue
    return ImageFont.load_default()


def main():
    icon = Image.open(ICON)
    accent = BRAND_ACCENT or accent_from_icon(icon)
    print(f"accent = {accent}")

    banner = make_panel(164, 314, accent, dots=True, glow_r=95)
    banner = paste_icon(banner, icon, 96, 82, 104)
    d = ImageDraw.Draw(banner)
    tw = d.textbbox((0, 0), "Dragent", font=load_font(26))[2]
    d.text(((164 - tw) / 2, 176), "Dragent", font=load_font(26), fill=(255, 255, 255))
    banner.save(f"{OUT_DIR}/dragent-banner.bmp")

    logo = make_panel(55, 58, accent, dots=False, glow_r=30)
    logo = paste_icon(logo, icon, 38, 27, 29)
    logo.save(f"{OUT_DIR}/dragent-logo.bmp")

    for f in ("dragent-banner.bmp", "dragent-logo.bmp"):
        im = Image.open(f"{OUT_DIR}/{f}")
        print(f"{f}: {im.size} mode={im.mode}")


if __name__ == "__main__":
    main()
