"""Cuts a 7 x 4 icon sheet (1536 x 1024, background #1A1A1A; grey, green and
amber ink) into 96 px transparent PNGs, a dark set and a recoloured light set,
plus a check sheet at small sizes.

Usage: python slice_glyphs.py <sheet.png> <out-dir> [round]
  round: "glyphs" (the 28 glyphs, sheet-dark.png) or "marks" (the 7 marks,
  marks-dark.png). Cells are read left to right, top to bottom, in its order."""
import os
import sys

from PIL import Image, ImageDraw, ImageFont

ROUNDS = {
    "glyphs": ["thermometer", "gauge", "clock", "bolt", "wave", "sliders", "shield-check",
               "trend", "info", "fan", "monitor", "chip", "graphics-card", "hexagon",
               "motherboard", "memory-stick", "drive", "battery", "network", "search", "up",
               "refresh", "admin-shield", "internet", "program", "record", "stop", "done"],
    "marks": ["chevron-down", "chevron-right", "sort-up", "sort-down", "plus", "arrow-right",
              "warning"],
}
BG = (26, 26, 26)
# Each ink in the dark set (as drawn on the sheet) and in the light set.
DARK = {"grey": (200, 200, 200), "green": (82, 255, 140), "amber": (0xF0, 0xB4, 0x2C)}
LIGHT = {"grey": (0x3D, 0x3D, 0x3D), "green": (0x08, 0x71, 0x3A), "amber": (0x82, 0x54, 0x00)}
OPTICAL = 88  # the geometric mean of an icon's box, in px of the 96 px canvas
MAX_SIDE = 94


def unmix(p):
    """The ink of pixel `p` and how much of it covers the background (0..1)."""
    if p[0] - p[2] > 60 and p[0] > p[1] + 15:
        ink = "amber"
    elif p[1] - p[0] > 25:
        ink = "green"
    else:
        ink = "grey"
    fg = DARK[ink]
    k = max(range(3), key=lambda c: abs(fg[c] - BG[c]))
    a = max(0.0, min(1.0, (p[k] - BG[k]) / (fg[k] - BG[k])))
    return ink, (0.0 if a < 0.04 else a)


def cut(sheet, out, names):
    src = Image.open(sheet).convert("RGB")
    cw, ch = src.width / 7, src.height / 4
    for s in ("dark", "light"):
        os.makedirs(os.path.join(out, s), exist_ok=True)
    for i, name in enumerate(names):
        r, c = divmod(i, 7)
        cell = src.crop((int(c * cw), int(r * ch), int((c + 1) * cw), int(r * ch + 180)))
        sets = {s: Image.new("RGBA", cell.size) for s in ("dark", "light")}
        px = {s: im.load() for s, im in sets.items()}
        cp = cell.load()
        for y in range(cell.height):
            for x in range(cell.width):
                ink, a = unmix(cp[x, y])
                alpha = int(round(a * 255))
                px["dark"][x, y] = (*DARK[ink], alpha)
                px["light"][x, y] = (*LIGHT[ink], alpha)
        bb = sets["dark"].split()[3].getbbox()
        w, h = bb[2] - bb[0], bb[3] - bb[1]
        scale = min(OPTICAL / (w * h) ** 0.5, MAX_SIDE / max(w, h))
        size = (max(1, round(w * scale)), max(1, round(h * scale)))
        for s, im in sets.items():
            icon = im.crop(bb).resize(size, Image.LANCZOS)
            final = Image.new("RGBA", (96, 96), (0, 0, 0, 0))
            final.paste(icon, ((96 - size[0]) // 2, (96 - size[1]) // 2))
            final.save(os.path.join(out, s, f"{name}.png"))


def check(out, names, path, small=(16, 20)):
    font = ImageFont.truetype(r"C:\Windows\Fonts\segoeui.ttf", 13)
    cw, ch = 150, 150
    sheet = Image.new("RGB", (7 * cw, ((len(names) + 6) // 7) * ch), BG)
    d = ImageDraw.Draw(sheet)
    a, b = small
    for i, n in enumerate(names):
        r, c = divmod(i, 7)
        x, y = c * cw, r * ch
        dk = Image.open(os.path.join(out, "dark", n + ".png"))
        lt = Image.open(os.path.join(out, "light", n + ".png"))
        for img, at in ((dk.resize((64, 64), Image.LANCZOS), (x + 10, y + 8)),
                        (dk.resize((a, a), Image.LANCZOS), (x + 86, y + 16)),
                        (dk.resize((b, b), Image.LANCZOS), (x + 110, y + 14))):
            sheet.paste(img, at, img)
        d.rectangle([x + 84, y + 44, x + 132, y + 66], fill=(232, 232, 232))
        for img, at in ((lt.resize((a, a), Image.LANCZOS), (x + 90, y + 47)),
                        (lt.resize((b, b), Image.LANCZOS), (x + 110, y + 45))):
            sheet.paste(img, at, img)
        d.text((x + 10, y + 80), f"{i + 1:02d} {n}", fill=(200, 200, 200), font=font)
    sheet.resize((sheet.width * 2, sheet.height * 2), Image.NEAREST).save(path)


if __name__ == "__main__":
    which = sys.argv[3] if len(sys.argv) > 3 else "glyphs"
    names = ROUNDS[which]
    cut(sys.argv[1], sys.argv[2], names)
    small = (12, 16) if which == "marks" else (16, 20)
    check(sys.argv[2], names, os.path.join(sys.argv[2], f"check-{which}-x2.png"), small)
    print("ok")
