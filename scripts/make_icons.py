"""Generates the app icon source and the tray icons: a runway seen from the threshold.

    python3 scripts/make_icons.py        (needs Pillow)
    npx tauri icon assets/app-icon.png   (expands the app icon into src-tauri/icons)

Tray icons follow docs/design/MenubarIcon.dc.html: the glyph is a runway narrowing to the
horizon, drawn on a 16 pt grid with a 1.5 pt stroke. Warning adds an amber triangle badge and
critical a red filled dot, top right, knocked out of the glyph (the glyph's top edge is shortened
to make room, as in the design).

- macOS (`tray-mac*.png`, 18 pt tall @2x): normal is a template image (black + alpha, tinted by
  macOS for light and dark menu bars); warning and critical are coloured.
- Windows and Linux (`tray-win*.png`, 32 px): a light glyph on a dark rounded tile, which reads on
  light and dark taskbars alike.
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


def _seg(d, a, b, w, ink):
    """A line with round caps."""
    d.line([a, b], fill=ink, width=int(round(w)))
    r = w / 2
    for (x, y) in (a, b):
        d.ellipse([x - r, y - r, x + r, y + r], fill=ink)


def runway_glyph(d, k, ox, oy, ink, badge, stroke=1.5):
    """The design's SVG paths in a 16-unit box at scale k (px per unit), offset (ox, oy)."""
    P = lambda x, y: (ox + x * k, oy + y * k)
    top_end = 10.5 if badge else 12.5
    _seg(d, P(3.5, 2.75), P(top_end, 2.75), stroke * k, ink)
    _seg(d, P(6.6, 2.75), P(2.6, 14.25), stroke * k, ink)
    _seg(d, P(9.4, 2.75), P(13.4, 14.25), stroke * k, ink)
    for y0, y1 in ((6, 7.3), (9.4, 10.9), (13, 14.25)):
        _seg(d, P(8, y0), P(8, y1), 1.2 * k, ink)


def badge(im, k, ox, oy, kind, colour):
    """Triangle (warning) or dot (critical) at the top right, with a transparent knock-out ring."""
    W, H = im.size
    mask = Image.new('L', (W, H), 0)
    md = ImageDraw.Draw(mask)
    shape = Image.new('L', (W, H), 0)
    sd = ImageDraw.Draw(shape)
    P = lambda x, y: (ox + x * k, oy + y * k)
    if kind == 'warn':
        tri = [P(14, 0.8), P(17.6, 7.0), P(10.4, 7.0)]
        # The knock-out: the same triangle grown by 1.2 units.
        cx, cy = sum(p[0] for p in tri) / 3, sum(p[1] for p in tri) / 3
        grow = [(cx + (x - cx) * 1.45, cy + (y - cy) * 1.45) for x, y in tri]
        md.polygon(grow, fill=255)
        sd.polygon(tri, fill=255)
    else:
        cx, cy, r = 14.2, 3.6, 3.1
        md.ellipse([*P(cx - r - 1.2, cy - r - 1.2), *P(cx + r + 1.2, cy + r + 1.2)], fill=255)
        sd.ellipse([*P(cx - r, cy - r), *P(cx + r, cy + r)], fill=255)
    clear = Image.new('RGBA', (W, H), (0, 0, 0, 0))
    im.paste(clear, (0, 0), mask)
    im.paste(Image.new('RGBA', (W, H), colour), (0, 0), shape)


def tray_state(kind, ink, colour=None, tile=None, px_h=36, s=8):
    """One tray icon. `kind`: None, 'warn' or 'crit'. Rendered at s× and scaled down."""
    vb_w = 18 if kind else 16
    if tile:
        W = H = px_h * s
        k = (px_h - 8) / 16 * s
        ox, oy = (W - vb_w * k) / 2, (H - 16 * k) / 2
    else:
        # 18 pt tall @2x: the 16-unit glyph with 1 pt of room above and below.
        k = (px_h - 4) / 16 * s
        H = px_h * s
        W = int(vb_w * k + 4 * s)
        ox, oy = 2 * s, 2 * s
    im = Image.new('RGBA', (int(W), int(H)), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    if tile:
        d.rounded_rectangle([0, 0, W - 1, H - 1], radius=int(W * 0.22), fill=tile)
    runway_glyph(d, k, ox, oy, ink, kind is not None)
    if kind:
        badge(im, k, ox, oy, kind, colour)
        if tile:
            # The knock-out cleared the tile too; put the tile back under it.
            under = Image.new('RGBA', im.size, (0, 0, 0, 0))
            ImageDraw.Draw(under).rounded_rectangle([0, 0, W - 1, H - 1], radius=int(W * 0.22), fill=tile)
            under.alpha_composite(im)
            im = under
    return im.resize((int(W // s), int(H // s)), Image.LANCZOS)




if __name__ == '__main__':
    (ROOT / 'assets').mkdir(exist_ok=True)
    (ROOT / 'src-tauri/icons').mkdir(parents=True, exist_ok=True)
    if not (ROOT / 'assets/app-icon.png').exists() or '--app' in __import__('sys').argv:
        app_icon().save(ROOT / 'assets/app-icon.png')
    out = ROOT / 'src-tauri/icons'
    tray_state(None, (0, 0, 0, 255)).save(out / 'tray-mac.png')
    tray_state('warn', (240, 160, 32, 255), (240, 160, 32, 255)).save(out / 'tray-mac-warn.png')
    tray_state('crit', (229, 72, 77, 255), (229, 72, 77, 255)).save(out / 'tray-mac-crit.png')
    tile, light = (28, 33, 48, 255), (238, 241, 247, 255)
    tray_state(None, light, tile=tile, px_h=32).save(out / 'tray-win.png')
    tray_state('warn', light, (245, 165, 36, 255), tile=tile, px_h=32).save(out / 'tray-win-warn.png')
    tray_state('crit', light, (255, 107, 117, 255), tile=tile, px_h=32).save(out / 'tray-win-crit.png')
    print('wrote assets/app-icon.png and src-tauri/icons/tray-{mac,win}{,-warn,-crit}.png')
