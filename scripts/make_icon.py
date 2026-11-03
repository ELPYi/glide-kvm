"""Draws glide's icon and writes every file that uses it.

    python scripts/make_icon.py

Writes assets/icon/glide.ico (embedded in glide.exe by build.rs, for the
taskbar and Explorer), glide-256.png (the window and Mac Dock icon) and
glide-32.png / glide-64.png (the tray). All are committed, so builds don't
need Python; rerun this after changing the art.

The art: a teal dragon gliding to the right, wings swept up and back,
with brown horns and an amber eye, on a dark tile. It's the same drawing at
every size, so the taskbar, tray and app header all match.
"""

import os
import sys

from PIL import Image, ImageDraw

INK = (0x14, 0x17, 0x1C)
EDGE = (0x3E, 0x55, 0x52)
MANE = (0x1E, 0x33, 0x33)
BODY = (0x4E, 0x86, 0x7E)
WING = (0x86, 0xC4, 0xB8)
FAR_WING = (0x2F, 0x5C, 0x57)
BELLY = (0xB4, 0xD6, 0xCC)
HORN = (0x7A, 0x48, 0x16)
HORN_LIT = (0xA8, 0x6C, 0x26)
EYE = (0xE0, 0x9A, 0x0C)

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
SS = 8  # supersampling

# The dragon is drawn on a 256 grid, then scaled about the centre and nudged
# so the wing tips and tail stay inside the tile.
SCALE, DX, DY = 0.84, -2.0, 12.0


def fit(p):
    return (128 + (p[0] - 128) * SCALE + DX, 128 + (p[1] - 128) * SCALE + DY)


def bezier(pts, t):
    while len(pts) > 1:
        pts = [((1 - t) * a[0] + t * b[0], (1 - t) * a[1] + t * b[1]) for a, b in zip(pts, pts[1:])]
    return pts[0]


def stroke(d, u, pts, r0, r1, fill, steps=80):
    """A tapered stroke along a bezier: radius r0 at the start, r1 at the end."""
    for i in range(steps + 1):
        t = i / steps
        x, y = fit(bezier(pts, t))
        r = (r0 + (r1 - r0) * t) * SCALE * u
        d.ellipse([x * u - r, y * u - r, x * u + r, y * u + r], fill=fill)


def poly(d, u, pts, fill):
    d.polygon([(fit(p)[0] * u, fit(p)[1] * u) for p in pts], fill=fill)


def dragon(d, u):
    # Far wing, darker, just behind the near one.
    poly(d, u, [(150, 116), (170, 66), (146, 30), (96, 18), (110, 48), (100, 50), (116, 78), (106, 84), (126, 114)], FAR_WING)
    # Tail, sweeping back down-left, ending in a spade.
    stroke(d, u, [(110, 146), (72, 160), (52, 186), (34, 204)], 13, 3, BODY)
    poly(d, u, [(40, 198), (16, 200), (26, 212), (24, 230), (40, 210)], MANE)
    # Body and belly.
    stroke(d, u, [(104, 146), (134, 138), (160, 124)], 21, 17, BODY)
    stroke(d, u, [(110, 158), (138, 152), (162, 138)], 9, 7, BELLY)
    # Legs tucked under.
    stroke(d, u, [(118, 160), (110, 172), (98, 176)], 6, 4, BODY)
    stroke(d, u, [(152, 140), (150, 154), (140, 160)], 5, 3, BODY)
    # Neck up to the head.
    stroke(d, u, [(156, 126), (178, 116), (190, 98)], 15, 11, BODY)
    # Head: skull, then a snout pointing right and a little down.
    stroke(d, u, [(186, 90), (206, 90), (228, 98)], 15, 8, BODY)
    stroke(d, u, [(196, 102), (214, 106), (226, 104)], 5, 3, BELLY)
    # Mane spikes down the neck and back.
    for x, y, s in [(178, 92, 11), (168, 104, 10), (156, 114, 9), (142, 122, 8), (128, 128, 7)]:
        poly(d, u, [(x - s * 0.9, y + s * 0.5), (x - s * 1.8, y - s * 0.8), (x + s * 0.7, y - s * 0.2)], MANE)
    # Horns: two, sweeping back from the skull and curling up at the tips.
    stroke(d, u, [(190, 82), (172, 70), (156, 66), (146, 50)], 9, 2.5, HORN)
    stroke(d, u, [(200, 80), (188, 60), (178, 50), (174, 30)], 9, 2.5, HORN_LIT)
    # Eye.
    stroke(d, u, [(208.5, 88.5)], 5, 5, EYE, steps=1)
    # Near wing: arm bone up from the shoulder, membrane scalloped back to the body.
    poly(d, u, [(140, 124), (156, 78), (136, 42), (78, 28), (96, 62), (84, 66), (104, 94), (92, 100), (114, 128)], WING)
    stroke(d, u, [(142, 124), (156, 78), (136, 42), (78, 28)], 6, 2, BODY)


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
    dragon(d, u)
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
