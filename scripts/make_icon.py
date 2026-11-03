"""Draws glide's icon and writes every file that uses it.

    python scripts/make_icon.py

Writes assets/icon/glide.ico (embedded in glide.exe by build.rs, for the
taskbar and Explorer), glide-256.png (the window and Mac Dock icon) and
glide-32.png / glide-64.png (the tray). All are committed, so builds don't
need Python; rerun this after changing the art.

The art: two screens side by side on the app's ink tile, and a signal-lime
cursor gliding across the gap between them, speed lines behind it. It's the
same drawing at every size, so the taskbar, tray and app header all match.
Palette from src/gui/theme.rs.
"""

import math
import os
import sys

from PIL import Image, ImageDraw

INK = (0x11, 0x11, 0x10)
LEFT_SCREEN = (0x31, 0x31, 0x2C)
RIGHT_SCREEN = (0x24, 0x24, 0x21)
EDGE = (0x5E, 0x5D, 0x55)
ACCENT = (0xC8, 0xF2, 0x5C)
TRAIL = (0x8A, 0xA8, 0x3E)

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
SS = 8  # supersampling


def dart(cx, cy, length, angle):
    """A cursor-style dart pointing along `angle` (radians), tip ahead of (cx, cy)."""
    pts = [(0.62, 0.0), (-0.38, -0.50), (-0.16, 0.0), (-0.38, 0.50)]
    c, s = math.cos(angle), math.sin(angle)
    return [(cx + (x * c - y * s) * length, cy + (x * s + y * c) * length) for x, y in pts]


def draw(size):
    n = size * SS
    u = n / 256.0  # design units: a 256 grid
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # The tile, with an edge so it reads on a dark taskbar too.
    pad = 8 * u
    d.rounded_rectangle([pad, pad, n - pad, n - pad], radius=44 * u, fill=EDGE)
    edge = max(SS, 4 * u)
    d.rounded_rectangle(
        [pad + edge, pad + edge, n - pad - edge, n - pad - edge], radius=44 * u - edge, fill=INK
    )

    # Two screens with a gap between them.
    top, bottom = 62 * u, 178 * u
    gap, mid = 16 * u, 128 * u
    d.rounded_rectangle([40 * u, top, mid - gap / 2, bottom], radius=12 * u, fill=LEFT_SCREEN)
    d.rounded_rectangle([mid + gap / 2, top, 216 * u, bottom], radius=12 * u, fill=RIGHT_SCREEN)
    # Stands.
    d.rounded_rectangle([64 * u, 190 * u, mid - gap / 2 - 24 * u, 200 * u], radius=5 * u, fill=LEFT_SCREEN)
    d.rounded_rectangle([mid + gap / 2 + 24 * u, 190 * u, 192 * u, 200 * u], radius=5 * u, fill=RIGHT_SCREEN)

    # Speed lines, then the dart crossing the gap.
    # (offset from the dart's line, start x, end x, thickness)
    lines = [(-28, 70, 112, 16), (0, 52, 118, 18), (28, 78, 112, 16)]
    cy = 122 * u
    for dy, x0, x1, width in lines:
        y = cy + dy * u
        d.rounded_rectangle([x0 * u, y - width * u / 2, x1 * u, y + width * u / 2], radius=width * u / 2, fill=TRAIL)
    d.polygon(dart(160 * u, cy, 108 * u, math.radians(-10)), fill=ACCENT)

    return img.resize((size, size), Image.LANCZOS)


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "icon")
    os.makedirs(root, exist_ok=True)
    images = {s: draw(s) for s in SIZES}
    for s in (32, 64, 256):
        images[s].save(os.path.join(root, f"glide-{s}.png"))
    images[256].save(
        os.path.join(root, "glide.ico"),
        sizes=[(s, s) for s in SIZES],
        append_images=[images[s] for s in SIZES if s != 256],
    )
    if "--preview" in sys.argv:
        # Every size at 1:1 and 4x, on a light and a dark strip.
        w = sum(SIZES) + 10 * len(SIZES) + 10
        sheet = Image.new("RGBA", (w * 2, 300), (0, 0, 0, 0))
        for row, bg in enumerate([(243, 243, 243, 255), (32, 32, 32, 255)]):
            strip = Image.new("RGBA", (w, 150), bg)
            x = 10
            for s in SIZES:
                strip.alpha_composite(images[s], (x, 10))
                x += s + 10
            sheet.paste(strip, (0, row * 150))
        big = Image.new("RGBA", (w, 300), (32, 32, 32, 255))
        x = 10
        for s in [16, 24, 32, 48]:
            z = images[s].resize((s * 4, s * 4), Image.NEAREST)
            big.alpha_composite(z, (x, 10))
            x += s * 4 + 12
        sheet.paste(big, (w, 0))
        sheet.save(sys.argv[sys.argv.index("--preview") + 1])


if __name__ == "__main__":
    main()
