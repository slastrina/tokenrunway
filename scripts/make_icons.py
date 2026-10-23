"""Generates the app icon source and the tray icon: a runway seen from the threshold.

    python3 scripts/make_icons.py        (needs Pillow)
    npx tauri icon assets/app-icon.png   (expands the app icon into src-tauri/icons)
"""
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent


def runway(d, W, top, bottom, ink, dash, lines=True):
    """A trapezoid narrowing towards the horizon, with centre-line dashes."""
    cx = W / 2
    tw, bw = W * 0.10, W * 0.36
    d.polygon([(cx - tw, top), (cx + tw, top), (cx + bw, bottom), (cx - bw, bottom)], fill=ink)
    if lines:
        n = 4
        for i in range(n):
            t0, t1 = (i + 0.25) / n, (i + 0.65) / n
            y0, y1 = top + (bottom - top) * t0, top + (bottom - top) * t1
            w0, w1 = W * (0.008 + 0.022 * t0), W * (0.008 + 0.022 * t1)
            d.polygon([(cx - w0, y0), (cx + w0, y0), (cx + w1, y1), (cx - w1, y1)], fill=dash)


def app_icon(size=1024):
    s = 4
    W = size * s
    im = Image.new('RGBA', (W, W), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    m = int(W * 0.09)
    d.rounded_rectangle([m, m, W - m, W - m], radius=int(W * 0.2), fill='#0d1321')
    # Gauge arc above the horizon: how much runway is left.
    r = W * 0.30
    box = [W / 2 - r, W * 0.40 - r, W / 2 + r, W * 0.40 + r]
    d.arc(box, 200, 340, fill='#2a3550', width=int(W * 0.045))
    d.arc(box, 200, 300, fill='#f5a524', width=int(W * 0.045))
    # The runway runs off the bottom edge, clipped to the rounded square.
    road = Image.new('RGBA', (W, W), (0, 0, 0, 0))
    runway(ImageDraw.Draw(road), W, W * 0.42, W - m, '#e8edf5', '#0d1321')
    mask = Image.new('L', (W, W), 0)
    ImageDraw.Draw(mask).rounded_rectangle([m, m, W - m, W - m], radius=int(W * 0.2), fill=255)
    im.paste(road, (0, 0), Image.composite(road, Image.new('RGBA', (W, W)), mask).getchannel('A'))
    return im.resize((size, size), Image.LANCZOS)


def tray(px=44):
    """Template image (black + alpha) so macOS tints it for light and dark menubars."""
    s = 8
    W = px * s
    im = Image.new('RGBA', (W, W), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    runway(d, W, W * 0.12, W * 0.92, (0, 0, 0, 255), (0, 0, 0, 0))
    return im.resize((px, px), Image.LANCZOS)


if __name__ == '__main__':
    (ROOT / 'assets').mkdir(exist_ok=True)
    (ROOT / 'src-tauri/icons').mkdir(parents=True, exist_ok=True)
    app_icon().save(ROOT / 'assets/app-icon.png')
    tray().save(ROOT / 'src-tauri/icons/tray.png')
    print('wrote assets/app-icon.png and src-tauri/icons/tray.png')
