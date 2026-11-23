"""Draws the Steam library artwork from the Maku logo.

Usage: python3 assets/steam/gen.py   (from the repo root; needs fontTools)

Writes capsule.svg, wide.svg, hero.svg and logo.svg next to this file.
The wordmark is Inter SemiBold, turned into outlines here, so the SVG
files need no font. Render them to PNG with render.js.
"""

import os
import re

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
MARK = os.path.join(ROOT, "assets", "logo", "urakata.svg")
FONT = os.path.join(ROOT, "crates", "urakata-app", "ui", "fonts", "Inter-SemiBold.ttf")

TEXT = "#ece9f6"
WORD = "urakata"


def mark_parts():
    """The defs and the drawing of the logo, without the outer <svg>."""
    src = open(MARK).read()
    inner = re.sub(r"^<svg[^>]*>", "", src.strip())
    inner = re.sub(r"</svg>$", "", inner)
    defs = re.search(r"<defs>.*?</defs>", inner, re.S).group(0)
    body = inner.replace(defs, "")
    return defs, body


def mark(x, y, size):
    """The logo (viewBox 256) at (x, y), `size` px wide."""
    s = size / 256
    return f'<g transform="translate({x:.1f} {y:.1f}) scale({s:.4f})">{BODY}</g>'


def word(x, baseline, height, anchor="start"):
    """The wordmark as one path, `height` px cap height (Inter's cap height
    is 0.727 em). Returns the path and its width."""
    font = TTFont(FONT)
    upem = font["head"].unitsPerEm
    em = height / 0.727
    scale = em / upem
    cmap = font.getBestCmap()
    glyphs = font.getGlyphSet()
    hmtx = font["hmtx"]
    width = sum(hmtx[cmap[ord(c)]][0] for c in WORD) * scale
    if anchor == "middle":
        x -= width / 2
    pen = SVGPathPen(glyphs)
    cur = 0
    for c in WORD:
        name = cmap[ord(c)]
        tp = TransformPen(pen, (scale, 0, 0, -scale, x + cur * scale, baseline))
        glyphs[name].draw(tp)
        cur += hmtx[name][0]
    return f'<path d="{pen.getCommands()}" fill="{TEXT}"/>', width


DEFS, BODY = mark_parts()

# Stage light: the magenta-to-cyan glow of the open curtain, spilled on the
# black ground behind the mark.
GROUND_DEFS = """
  <radialGradient id="glowM" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#ff00ff" stop-opacity="0.30"/><stop offset="1" stop-color="#ff00ff" stop-opacity="0"/></radialGradient>
  <radialGradient id="glowC" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#03edf9" stop-opacity="0.22"/><stop offset="1" stop-color="#03edf9" stop-opacity="0"/></radialGradient>
  <linearGradient id="floor" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#ff00ff" stop-opacity="0"/><stop offset="0.5" stop-color="#9a8bff" stop-opacity="0.55"/><stop offset="1" stop-color="#03edf9" stop-opacity="0"/></linearGradient>
  <linearGradient id="drapeL" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#140d1e"/><stop offset="1" stop-color="#2e2142"/></linearGradient>
"""


def svg(w, h, content, ground=True):
    bg = f'<rect width="{w}" height="{h}" fill="#000"/>' if ground else ""
    defs = DEFS.replace("</defs>", GROUND_DEFS + "</defs>")
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" '
            f'viewBox="0 0 {w} {h}">{defs}{bg}{content}</svg>\n')


def glow(cx, cy, r):
    """Two offset glows: magenta up left, cyan down right, as on the mark."""
    d = r * 0.28
    return (f'<ellipse cx="{cx - d:.0f}" cy="{cy - d * 0.6:.0f}" rx="{r:.0f}" ry="{r * 0.8:.0f}" fill="url(#glowM)"/>'
            f'<ellipse cx="{cx + d:.0f}" cy="{cy + d * 0.6:.0f}" rx="{r:.0f}" ry="{r * 0.8:.0f}" fill="url(#glowC)"/>')


def floor(cx, y, w):
    return f'<rect x="{cx - w / 2:.0f}" y="{y:.0f}" width="{w:.0f}" height="2" fill="url(#floor)"/>'


def drape(w, h, x, mirror):
    """A stage curtain at one side of the hero, with soft folds."""
    path = (f'M0 0 H{x} C{x * 0.92:.0f} {h * 0.35:.0f} {x * 0.70:.0f} {h * 0.70:.0f} '
            f'{x * 0.42:.0f} {h} H0 Z')
    folds = "".join(
        f'<path d="M{x * f:.0f} 0 C{x * f * 0.95:.0f} {h * 0.4:.0f} {x * f * 0.75:.0f} {h * 0.75:.0f} {x * f * 0.45:.0f} {h}" '
        f'fill="none" stroke="#ffffff" stroke-opacity="0.05" stroke-width="6"/>'
        for f in (0.78, 0.52, 0.28))
    g = f'<path d="{path}" fill="url(#drapeL)"/>{folds}'
    if mirror:
        return f'<g transform="translate({w} 0) scale(-1 1)">{g}</g>'
    return g


def capsule():
    w, h = 600, 900
    m = 380
    mx, my = (w - m) / 2, 210
    t, _ = word(w / 2, 720, 70, "middle")
    c = glow(w / 2, my + m * 0.45, 330) + mark(mx, my, m) + floor(w / 2, my + m + 18, 420) + t
    return svg(w, h, c)


def wide():
    w, h = 920, 430
    m = 280
    t, tw = word(0, 0, 76)
    gap = 44
    total = m + gap + tw
    x0 = (w - total) / 2
    mx, my = x0, (h - m) / 2 - 6
    t, _ = word(x0 + m + gap, h / 2 + 76 / 2 + 6, 76)
    c = glow(mx + m / 2, my + m * 0.45, 260) + mark(mx, my, m) + floor(w / 2, my + m + 10, 640) + t
    return svg(w, h, c)


def hero():
    w, h = 1920, 620
    c = (glow(w / 2, h * 0.42, 820)
         + f'<rect x="0" y="{h - 120}" width="{w}" height="120" fill="#000" opacity="0.55"/>'
         + floor(w / 2, h - 118, 1500)
         + drape(w, h, 300, False) + drape(w, h, 300, True))
    return svg(w, h, c)


def logo():
    """Transparent, cut close to the drawing: Steam fits the logo into a box
    over the hero, so empty margins only make it smaller."""
    m, gap, pad = 340, 56, 8
    t, tw = word(0, 0, 120)
    w, h = round(m + gap + tw + 2 * pad), m + 2 * pad
    x0 = pad
    my = pad
    t, _ = word(x0 + m + gap, h / 2 + 120 / 2 + 6, 120)
    return svg(w, h, mark(x0, my, m) + t, ground=False)


for name, fn in {"capsule.svg": capsule, "wide.svg": wide, "hero.svg": hero, "logo.svg": logo}.items():
    open(os.path.join(HERE, name), "w").write(fn())
