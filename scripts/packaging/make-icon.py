"""Regenerate the Aquanter Surface app icons in assets/app-icon/.

Usage: python scripts/packaging/make-icon.py   (needs Pillow)
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parents[2] / "assets" / "app-icon"
SS = 4  # supersampling factor
S = 1024 * SS


def px(v):
    return v * SS


def wave(cy, amp, phase):
    pts = []
    for i in range(1201):
        x = 210 + 604 * i / 1200
        y = cy - amp * math.sin(2 * math.pi * (1.15 * i / 1200) + phase)
        pts.append((px(x), px(y)))
    return pts


def render():
    # background: deep-ocean vertical gradient inside a squircle
    bg = Image.new("RGBA", (S, S))
    top, bottom = (16, 42, 68), (5, 16, 30)
    grad = Image.new("RGBA", (1, S))
    for y in range(S):
        t = y / (S - 1)
        grad.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,))
    grad = grad.resize((S, S))
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (px(40), px(40), px(984), px(984)), radius=px(230), fill=255
    )
    bg.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(bg)
    w = px(62)
    # three water layers, brightest on the surface
    layers = [
        (700, 34, 1.1, (27, 92, 150)),
        (555, 40, 0.55, (24, 150, 196)),
        (410, 46, 0.0, (64, 232, 212)),
    ]
    for cy, amp, phase, color in layers:
        pts = wave(cy, amp, phase)
        # ponytail: stamped discs instead of d.line, whose joints leave hairline cracks
        for x, y in pts:
            d.ellipse((x - w / 2, y - w / 2, x + w / 2, y + w / 2), fill=color + (255,))

    # live-price dot riding the surface
    x, y = wave(410, 46, 0.0)[-1]
    r = px(54)
    d.ellipse((x - r, y - r, x + r, y + r), fill=(5, 16, 30, 255))
    r = px(38)
    d.ellipse((x - r, y - r, x + r, y + r), fill=(235, 255, 252, 255))

    return bg.resize((1024, 1024), Image.LANCZOS)


def main():
    icon = render()
    (OUT / "linux").mkdir(parents=True, exist_ok=True)
    icon.save(OUT / "windows.png")
    icon.resize((512, 512), Image.LANCZOS).save(OUT / "linux" / "aquanter-surface.png")
    icon.save(
        OUT / "aquanter-surface.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    icon.save(OUT / "aquanter-surface.icns")
    (OUT / "windows-window.rgba").write_bytes(
        icon.resize((64, 64), Image.LANCZOS).convert("RGBA").tobytes()
    )


if __name__ == "__main__":
    main()
