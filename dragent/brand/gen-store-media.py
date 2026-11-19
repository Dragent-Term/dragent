#!/usr/bin/env python3
"""
Generate LemonSqueezy product media from the master brand art.

Output: 1652x1240 PNG (4:3) per product, into brand/store/.

Why 4:3 at this size: LemonSqueezy asks for 4:3 and recommends 1652x1240 for HD
previews. It then derives its own smaller thumbnails, and those are SQUARE —
which crops the SIDES of a 4:3 image, never the top or bottom. Every composition
here is therefore centred inside the middle 1240px (SAFE_X0..SAFE_X1); anything
outside that band is atmosphere only and must be safe to lose. Nothing legible
ever goes there.

The art derives from `dragent-avatar.svg` (same ribbon polygons, same colour
story) so store media, app icon, and socials cannot drift apart. Edit here and
rerun; never hand-edit the PNGs.

Render path is macOS-native. Note the canvas trick: qlmanage always emits a
SQUARE thumbnail, and for a non-square SVG it scales to fit the *height* and
crops the width from the left — so rendering a 1652x1240 source silently loses
the right quarter of the art. Authoring on a 1652x1652 canvas and cropping the
4:3 band back out with sips renders 1:1 with nothing lost. resvg would do this
in one step, but it is not installed and this avoids a new build dependency.

    python3 brand/gen-store-media.py
"""

import subprocess
import sys
from pathlib import Path

W, H = 1652, 1240  # final 4:3 output
SQ = 1652  # square authoring canvas (see module docstring)
BAND = (SQ - H) // 2  # 206 — vertical offset of the 4:3 band within the square

SAFE = 1240  # square-thumbnail crop width
SAFE_X0 = (W - SAFE) // 2  # 206
SAFE_X1 = SAFE_X0 + SAFE  # 1446

CX = W // 2  # 826 — also the square canvas's horizontal centre

# Ribbon geometry on the master 1024 grid (from dragent-avatar.svg).
MARK_BBOX_CX, MARK_BBOX_CY, MARK_BBOX_W = 513, 512, 602

MARK_POLYS = [
    ("814,512 389,812 389,670 606,512", "url(#fLo)", None),
    ("389,812 389,670 212,545 231,706", "url(#fTail)", None),
    ("389,212 814,512 606,512 389,354", "url(#fUp)", None),
    ("389,212 814,512 795,512 389,229", "#d6f9ff", "0.92"),
    ("389,670 212,545 218,556 389,681", "#8feee2", "0.55"),
]

# Layout, expressed in the FINAL 4:3 frame. `y()` shifts these onto the square
# canvas, so the numbers here can be read against the 1652x1240 deliverable.
MARK_CY, MARK_W = 380, 360
Y_EYEBROW = 672
Y_TIER = 866
Y_RULE = 934
Y_TAGLINE = 1018

# One entry per LemonSqueezy product. `aurora` tints the ground so tiers stay
# distinguishable at 100px, where the wordmark is far too small to read.
PRODUCTS = [
    {
        "slug": "pro",
        "name": "PRO",
        "aurora": "#19aad8",
        "word": [("0", "#9df5ff"), ("0.55", "#4fd4f6"), ("1", "#22b4e4")],
    },
    {
        "slug": "max",
        "name": "MAX",
        "aurora": "#2fd4c8",
        "word": [("0", "#c9fff8"), ("0.5", "#2fd4c8"), ("1", "#12a2bd")],
    },
    {
        "slug": "ultra",
        "name": "ULTRA",
        "aurora": "#6b5bd6",
        "word": [("0", "#d8e2ff"), ("0.5", "#8f9cff"), ("1", "#3a3f9e")],
    },
    {
        "slug": "credits",
        "name": "CREDITS",
        "aurora": "#19aad8",
        "word": [("0", "#eaf6ff"), ("0.6", "#a9c2d8"), ("1", "#7d99b2")],
    },
]

TAGLINE = "the self-healing terminal"

SANS = "Helvetica Neue, Helvetica, Arial, sans-serif"
MONO = "Menlo, monospace"


def y(v: int) -> int:
    """Map a y in the final 4:3 frame onto the square authoring canvas."""
    return v + BAND


def mark(cx: int, cy: int, width: int) -> str:
    """Ribbon mark scaled so its visual bounds are `width` wide, centred on cx/cy."""
    s = width / MARK_BBOX_W
    tx = cx - MARK_BBOX_CX * s
    ty = cy - MARK_BBOX_CY * s
    polys = "\n      ".join(
        f'<polygon points="{p}" fill="{f}"' + (f' opacity="{o}"' if o else "") + "/>"
        for p, f, o in MARK_POLYS
    )
    return (
        f'<g transform="translate({tx:.2f},{ty:.2f}) scale({s:.5f})">\n'
        f"      {polys}\n    </g>"
    )


def svg(product: dict) -> str:
    aurora = product["aurora"]
    stops = "".join(f'<stop offset="{o}" stop-color="{c}"/>' for o, c in product["word"])
    mark_cy = y(MARK_CY)

    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="{SQ}" height="{SQ}" viewBox="0 0 {SQ} {SQ}">
  <!-- Authored square; the shipped PNG is the centre {W}x{H} band (y {BAND}..{BAND + H}). -->
  <defs>
    <linearGradient id="sq" gradientUnits="userSpaceOnUse" x1="0" y1="{y(0)}" x2="0" y2="{y(H)}">
      <stop offset="0" stop-color="#16202e"/><stop offset="1" stop-color="#0a0e15"/>
    </linearGradient>
    <radialGradient id="aur1" gradientUnits="userSpaceOnUse" cx="{CX}" cy="{mark_cy}" r="1020">
      <stop offset="0" stop-color="{aurora}" stop-opacity="0.24"/>
      <stop offset="1" stop-color="{aurora}" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="aur2" gradientUnits="userSpaceOnUse" cx="1360" cy="{y(1120)}" r="900">
      <stop offset="0" stop-color="#6b5bd6" stop-opacity="0.16"/>
      <stop offset="1" stop-color="#6b5bd6" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="glow" gradientUnits="userSpaceOnUse" cx="{CX}" cy="{mark_cy}" r="430">
      <stop offset="0" stop-color="{aurora}" stop-opacity="0.32"/>
      <stop offset="0.6" stop-color="{aurora}" stop-opacity="0.10"/>
      <stop offset="1" stop-color="{aurora}" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="word" x1="0" y1="0" x2="1" y2="0.45">{stops}</linearGradient>
    <linearGradient id="rule" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="{aurora}" stop-opacity="0"/>
      <stop offset="0.5" stop-color="{aurora}" stop-opacity="0.7"/>
      <stop offset="1" stop-color="{aurora}" stop-opacity="0"/>
    </linearGradient>
    <linearGradient id="fUp" x1="0" y1="0" x2="1" y2="0.9">
      <stop offset="0" stop-color="#9df5ff"/>
      <stop offset="0.55" stop-color="#4fd4f6"/>
      <stop offset="1" stop-color="#22b4e4"/>
    </linearGradient>
    <linearGradient id="fLo" x1="0" y1="0" x2="0.9" y2="1">
      <stop offset="0" stop-color="#1587c0"/>
      <stop offset="0.6" stop-color="#0d5da4"/>
      <stop offset="1" stop-color="#3a3f9e"/>
    </linearGradient>
    <linearGradient id="fTail" x1="1" y1="1" x2="0" y2="0">
      <stop offset="0" stop-color="#0e6f9e"/>
      <stop offset="1" stop-color="#2fd4c8"/>
    </linearGradient>
  </defs>

  <rect width="{SQ}" height="{SQ}" fill="url(#sq)"/>
  <rect width="{SQ}" height="{SQ}" fill="url(#aur1)"/>
  <rect width="{SQ}" height="{SQ}" fill="url(#aur2)"/>

  <!-- Halo. Decorative: it bleeds past the square-crop band on purpose. -->
  <circle cx="{CX}" cy="{mark_cy}" r="430" fill="url(#glow)"/>

  {mark(CX, mark_cy, MARK_W)}

  <text x="{CX}" y="{y(Y_EYEBROW)}" text-anchor="middle" font-family="{SANS}"
        font-weight="500" font-size="42" letter-spacing="16" fill="#8299b0">DRAGENT</text>

  <text x="{CX}" y="{y(Y_TIER)}" text-anchor="middle" font-family="{SANS}"
        font-weight="700" font-size="158" letter-spacing="3" fill="url(#word)">{product['name']}</text>

  <rect x="{CX - 190}" y="{y(Y_RULE)}" width="380" height="2" fill="url(#rule)"/>

  <text x="{CX}" y="{y(Y_TAGLINE)}" text-anchor="middle" font-family="{MONO}"
        font-size="38" fill="#647f97">{TAGLINE}</text>
</svg>
"""


def main() -> int:
    brand = Path(__file__).resolve().parent
    out = brand / "store"
    out.mkdir(exist_ok=True)

    for product in PRODUCTS:
        slug = product["slug"]
        svg_path = out / f"dragent-{slug}.svg"
        svg_path.write_text(svg(product))

        subprocess.run(
            ["qlmanage", "-t", "-s", str(SQ), "-o", str(out), str(svg_path)],
            check=True,
            capture_output=True,
        )
        rendered = out / f"{svg_path.name}.png"
        final = out / f"dragent-{slug}.png"
        # Crop the square render down to the 4:3 band. sips -c is centred.
        subprocess.run(
            ["sips", "-c", str(H), str(W), str(rendered), "--out", str(final)],
            check=True,
            capture_output=True,
        )
        rendered.unlink()
        print(f"  {final.relative_to(brand.parent)}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
