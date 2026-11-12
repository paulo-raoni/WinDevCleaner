#!/usr/bin/env python3
"""Generate assets/windevcleaner.ico and assets/icon-256.png from the shapes of assets/icon.svg.

The shapes are drawn with Pillow (no SVG renderer needed), supersampled 8x and reduced with a
Lanczos filter, once per icon size, so each size is drawn rather than scaled from the 256 one.
Keep the geometry here in step with assets/icon.svg. Run from anywhere: python3 tools/make-icon.py
"""
from pathlib import Path

from PIL import Image, ImageDraw

SIZES = [16, 24, 32, 48, 64, 128, 256]
SS = 8  # supersampling factor

NAVY = (0x1D, 0x35, 0x57, 255)
PAPER = (0xF1, 0xFA, 0xEE, 255)
AMBER = (0xF4, 0xA3, 0x1B, 255)

STAR = [(190, 32), (198, 58), (224, 66), (198, 74), (190, 100), (182, 74), (156, 66), (182, 58)]


def draw(size: int) -> Image.Image:
    k = size * SS / 256.0
    big = Image.new("RGBA", (size * SS, size * SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)

    def box(cx, cy, r):
        return [(cx - r) * k, (cy - r) * k, (cx + r) * k, (cy + r) * k]

    d.rounded_rectangle([8 * k, 8 * k, 248 * k, 248 * k], radius=52 * k, fill=NAVY)
    d.ellipse(box(128, 128, 88), fill=PAPER)
    d.pieslice(box(128, 128, 88), start=-90, end=0, fill=AMBER)
    d.ellipse(box(128, 128, 20), fill=NAVY)
    d.polygon([(x * k, y * k) for x, y in STAR], fill=PAPER)
    return big.resize((size, size), Image.LANCZOS)


def main() -> None:
    out = Path(__file__).resolve().parent.parent / "assets"
    out.mkdir(exist_ok=True)
    images = {s: draw(s) for s in SIZES}
    images[256].save(out / "icon-256.png", optimize=False)
    images[256].save(
        out / "windevcleaner.ico",
        format="ICO",
        sizes=[(s, s) for s in SIZES],
        append_images=[images[s] for s in SIZES if s != 256],
    )


if __name__ == "__main__":
    main()
