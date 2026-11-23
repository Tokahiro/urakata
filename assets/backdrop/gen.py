"""Draws the backdrops of the settings window: a stage between two curtains, as on the
Steam hero. One for the desktop, two for Big Picture.

Usage: python3 assets/backdrop/gen.py   (from the repo root; needs numpy and Pillow)

Writes crates/urakata-app/ui/backdrop-desktop.png, baked as Kidō bakes the art behind a page
without art of its own: drawn large, made small, blurred, dimmed by half. Noise goes in before the
8-bit step, so the blur does not band. The bright parts bend down, so that `text-muted`
keeps 4.5:1 on it, also under a pressed sidebar item, wherever the window crops it and the
page scrolls. It reads the colours from kido.slint: run it again when they change.

Big Picture (seen on a stream) gets a sharper backdrop-bp.png for the bare window and a
blurred frost-bp.png that the glass shows inside its shape, so the glass reads as frosted glass
over the stage. Their contrast is checked as the stream shows them; then they are lifted by
v x 219 / 255 + 16, which the stream's range step takes back.
"""

import os
import re
import sys

import numpy as np
from PIL import Image, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
UI = os.path.join(ROOT, "crates", "urakata-app", "ui")
OUT = os.path.join(UI, "backdrop-desktop.png")

W, H = 1536, 1024
SMALL = (384, 256)
# Kidō blurs at sigma 6 on a 256 px wide copy; the same share of 384 px.
SIGMA = 9
DIM = 0.5
# Big Picture's frost, in linear light, after the bend: the bend keeps the brightest glass at
# 4.5:1 whatever the dim, so only a step after it makes the whole glass darker.
FROST_GAIN = 0.5
MIN_CONTRAST = 4.5

v, u = np.mgrid[0:H, 0:W].astype(np.float32)
u /= W
v /= H


def rgb(hexcode):
    return np.array([int(hexcode[i:i + 2], 16) for i in (1, 3, 5)], np.float32) / 255


def token(name):
    """A colour of the `Kido` global as (rgb, alpha)."""
    src = open(os.path.join(UI, "kido.slint")).read()
    value = re.search(rf"property <color> {name}: (#[0-9a-f]{{6}})([0-9a-f]{{2}})?;", src)
    return rgb(value[1]), int(value[2] or "ff", 16) / 255


def glow(cx, cy, rx, ry, color, power):
    d = ((u - cx) / rx) ** 2 + ((v - cy) / ry) ** 2
    return (np.exp(-d * 2.2) * power)[..., None] * rgb(color)


def curtain(img, left):
    """A drape at one side, lighter towards the stage, its inner edge curving out at the bottom."""
    edge = 0.17 + 0.10 * v**2.2 if left else 0.83 - 0.10 * v**2.2
    inside = u < edge if left else u > edge
    t = u / edge if left else (1 - u) / (1 - edge)
    cloth = rgb("#5a3f86") * (0.78 * (0.6 + 0.4 * t))[..., None]
    return np.where(inside[..., None], cloth, img)


def stage(floor=True):
    """The stage. Big Picture's has no floor: it shows the sharp backdrop between the frosted
    cards, where the dark floor read as dark strips under them."""
    img = np.zeros((H, W, 3), np.float32) + rgb("#140d1e")
    img += glow(0.36, 0.3, 0.5, 0.55, "#c42ad8", 1.1)
    img += glow(0.66, 0.55, 0.45, 0.5, "#18a3c4", 1.0)
    ground = rgb("#0a0710") + glow(0.5, 0.72, 0.5, 0.12, "#2a1a3a", 0.6)
    if floor:
        img = np.where((v > 0.72)[..., None], ground, img)
    return curtain(curtain(img, True), False)


def to_linear(c):
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def to_srgb(c):
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * np.maximum(c, 0) ** (1 / 2.4) - 0.055)


def luminance(c):
    """WCAG relative luminance of sRGB values from 0 to 1."""
    return to_linear(c) @ np.array([0.2126, 0.7152, 0.0722], np.float32)


def bake(img, limit, size=SMALL, sigma=SIGMA, dim=DIM, lift=False, gain=1.0):
    """`img` made small, blurred and dimmed, its luminance bent below `limit`. With `lift`,
    lifted for the stream after the bend, the contrast check reads it back with `unlift`."""
    sharp = Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8))
    small = sharp.resize(size, Image.LANCZOS).filter(ImageFilter.GaussianBlur(sigma))
    dimmed = np.asarray(small).astype(np.float32) / 255 * dim
    # A soft knee in linear light: the darker half stays as it is, the bright parts above it
    # bend smoothly towards `limit` and never reach it.
    knee = limit / 2
    lum = luminance(dimmed)
    bent = np.where(lum > knee, knee + (limit - knee) * (1 - np.exp(-(lum - knee) / (limit - knee))), lum)
    capped = to_srgb(to_linear(dimmed) * (gain * bent / np.maximum(lum, 1e-6))[..., None])
    if lift:
        capped = capped * 219 / 255 + 16 / 255
    noise = np.random.default_rng(1).normal(0, 0.6, capped.shape)
    return np.clip(np.round(capped * 255 + noise), 0, 255).astype(np.uint8)


def unlift(baked):
    """A Big Picture picture as the stream shows it, from 0 to 1."""
    return np.clip((baked.astype(np.float32) - 16) / 219, 0, 1)


def worst_contrast(baked):
    """`text-muted` on the brightest spot where it lies: bare (the notes under a page), and in
    the sidebar's glass at its clearest edge under the fill of a pressed item."""
    muted, _ = token("text-muted-desktop")
    glass, glass_alpha = token("lens-panel-to")
    press, press_alpha = token("press")
    bare = baked / 255
    pressed = (bare * (1 - glass_alpha) + glass * glass_alpha) * (1 - press_alpha) + press * press_alpha
    brightest = max(luminance(bare).max(), luminance(pressed).max())
    return (luminance(muted) + 0.05) / (brightest + 0.05)


def contrast(text, back):
    return (luminance(text) + 0.05) / (luminance(back).max() + 0.05)


def bp_bare(baked):
    """Notes on the bare Big Picture backdrop, as the stream shows it (`text-muted` there is
    `text-secondary`; this checks the darker one)."""
    return contrast(token("text-muted-desktop")[0], unlift(baked))


def bp_frost(baked):
    """The darkest text on the Big Picture glass: `text-muted`, which is `text-secondary` there
    (notes on cards, the sidebar items), on the frost under the tint at its alpha, also under a
    pressed item's fill."""
    src = open(os.path.join(UI, "kido.slint")).read()
    alpha = float(re.search(r"property <float> glass-alpha: ([0-9.]+);", src)[1])
    tint = (token("glass-top-stream")[0] * 255 - 16) / 219
    press, press_alpha = token("press")
    glass = unlift(baked) * (1 - alpha) + tint * alpha
    pressed = glass * (1 - press_alpha) + press * press_alpha
    muted = token("text-secondary")[0]
    return min(contrast(muted, glass), contrast(muted, pressed))


def write(out, check, what, drawn, limit=1.0, gain=1.0, **kw):
    """Bakes `drawn` with the luminance bent down until `check` gives 4.5:1, then scaled by
    `gain` (at most 1, so the contrast only grows), and writes it."""
    while check(baked := bake(drawn, limit, **kw)) < MIN_CONTRAST:
        limit *= 0.97
        if limit < 1e-4:
            sys.exit(f"{what} cannot keep 4.5:1: check kido.slint")
    baked = bake(drawn, limit, gain=gain, **kw)
    Image.fromarray(baked).save(out, optimize=True)
    print(f"{out}: {baked.shape[1]} x {baked.shape[0]}, {what} at worst {check(baked):.2f}:1")


write(OUT, worst_contrast, "text-muted", stage(),
      limit=(luminance(token("text-muted-desktop")[0]) + 0.05) / MIN_CONTRAST - 0.05)
bare = stage(floor=False)
write(os.path.join(UI, "backdrop-bp.png"), bp_bare, "text-muted (Big Picture)", bare,
      size=(768, 512), sigma=3, dim=0.8, lift=True)
write(os.path.join(UI, "frost-bp.png"), bp_frost, "text-muted (text-secondary) on the glass (Big Picture)", bare,
      sigma=SIGMA, dim=0.8, lift=True, gain=FROST_GAIN)
