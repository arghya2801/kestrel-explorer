"""Kestrel's app icon: a yellow folder with a gliding kestrel. Pure Python, no dependencies.
Usage: python make_icon.py app.ico [preview.png]
"""
import math
import struct
import sys
import zlib

EDGE = (176, 122, 18)
BACK = (232, 168, 36)
FRONT_TOP = (255, 214, 92)
FRONT_BOTTOM = (246, 188, 56)
BIRD = (52, 78, 120)          # slate blue, like a male kestrel's head and tail


def circle(cx, cy, r, n=40):
    return [(cx + r * math.cos(2 * math.pi * i / n), cy + r * math.sin(2 * math.pi * i / n)) for i in range(n)]


def rounded_rect(x0, y0, x1, y1, r, n=8):
    pts = []
    for (cx, cy, a0) in [(x1 - r, y0 + r, -90), (x1 - r, y1 - r, 0), (x0 + r, y1 - r, 90), (x0 + r, y0 + r, 180)]:
        for i in range(n + 1):
            a = math.radians(a0 + 90 * i / n)
            pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return pts


def mirror(poly):
    return [(1 - x, y) for (x, y) in reversed(poly)]


# Kestrel hovering, seen from the front, in a unit box (x right, y down).
LEFT_WING = [(0.46, 0.31), (0.33, 0.24), (0.17, 0.17), (0.03, 0.13), (0.10, 0.24),
             (0.17, 0.32), (0.27, 0.39), (0.38, 0.43), (0.46, 0.45)]
BODY = [(0.45, 0.26), (0.55, 0.26), (0.58, 0.44), (0.54, 0.62), (0.46, 0.62), (0.42, 0.44)]
TAIL = [(0.46, 0.58), (0.54, 0.58), (0.64, 0.85), (0.57, 0.90), (0.50, 0.91), (0.43, 0.90), (0.36, 0.85)]
HEAD = circle(0.5, 0.21, 0.075)
BIRD_FULL = [LEFT_WING, mirror(LEFT_WING), BODY, TAIL, HEAD]
BIRD_PLAIN = [LEFT_WING, mirror(LEFT_WING), BODY, TAIL]   # 32 px: no separate head
# Profile version: kestrel gliding, seen from the side, facing left. The wing
# sweeps up and back toward the tail; the tail fans out toward its tip.
P_HEAD = circle(0.22, 0.47, 0.075)
P_BEAK = [(0.16, 0.45), (0.08, 0.50), (0.12, 0.52), (0.17, 0.52)]
P_BODY = [(0.24, 0.40), (0.36, 0.41), (0.50, 0.44), (0.62, 0.49), (0.64, 0.56), (0.50, 0.58),
          (0.36, 0.57), (0.24, 0.54)]
P_TAIL = [(0.60, 0.49), (0.75, 0.52), (0.93, 0.53), (0.95, 0.60), (0.93, 0.68), (0.75, 0.62), (0.60, 0.57)]
P_UP_WING = [(0.36, 0.44), (0.40, 0.33), (0.47, 0.23), (0.57, 0.14), (0.68, 0.08), (0.82, 0.05),
             (0.74, 0.12), (0.67, 0.21), (0.62, 0.32), (0.58, 0.44)]
PROFILE_FULL = [P_UP_WING, P_BODY, P_TAIL, P_HEAD, P_BEAK]
PROFILE_PLAIN = [P_UP_WING, P_BODY, P_TAIL, P_HEAD]
PROFILE_TINY = [P_UP_WING, P_BODY, P_TAIL, circle(0.22, 0.47, 0.09)]
STYLE = "profile"

# 16 px: just the spread wings with a hint of body.
WINGS_ONLY = [[(0.02, 0.22), (0.50, 0.50), (0.98, 0.22), (0.98, 0.42), (0.58, 0.70), (0.50, 0.80),
               (0.42, 0.70), (0.02, 0.42)]]


def place(polys, cx, cy, size):
    return [[(cx + (x - 0.5) * size, cy + (y - 0.5) * size) for (x, y) in p] for p in polys]


def fill(cov, w, h, poly, s):
    """Marks samples inside `poly` (unit coords) in the s*w x s*h coverage grid."""
    W, H = w * s, h * s
    pts = [(x * W, y * H) for (x, y) in poly]
    ymin = max(0, int(min(p[1] for p in pts)))
    ymax = min(H - 1, int(max(p[1] for p in pts)) + 1)
    n = len(pts)
    for sy in range(ymin, ymax + 1):
        yc = sy + 0.5
        xs = []
        for i in range(n):
            (x0, y0), (x1, y1) = pts[i], pts[(i + 1) % n]
            if (y0 <= yc < y1) or (y1 <= yc < y0):
                xs.append(x0 + (yc - y0) * (x1 - x0) / (y1 - y0))
        xs.sort()
        row = cov[sy]
        for a, b in zip(xs[::2], xs[1::2]):
            for sx in range(max(0, int(a + 0.5)), min(W, int(b + 0.5))):
                row[sx] = 1


def layer(img, w, h, polys, color, s, gradient=None):
    """Paints `polys` onto RGBA float image with anti-aliasing (s x s samples per pixel)."""
    cov = [[0] * (w * s) for _ in range(h * s)]
    for p in polys:
        fill(cov, w, h, p, s)
    for y in range(h):
        if gradient:
            (c0, c1, y0, y1) = gradient
            t = min(1, max(0, (y / h - y0) / (y1 - y0)))
            col = tuple(c0[i] * (1 - t) + c1[i] * t for i in range(3))
        else:
            col = color
        for x in range(w):
            a = sum(cov[y * s + j][x * s + i] for j in range(s) for i in range(s)) / (s * s)
            if a:
                r, g, b, al = img[y][x]
                img[y][x] = (r * (1 - a) + col[0] * a, g * (1 - a) + col[1] * a, b * (1 - a) + col[2] * a,
                             al + (1 - al) * a)


def icon(size):
    s = 8 if size <= 48 else 3
    img = [[(0, 0, 0, 0)] * size for _ in range(size)]
    px = 1 / size  # one pixel in unit coords, for outlines
    # Folder: outline first, then fills inset by a pixel.
    layer(img, size, size, [rounded_rect(0.04, 0.10, 0.44, 0.26, 0.04)], EDGE, s)
    layer(img, size, size, [rounded_rect(0.04, 0.16, 0.96, 0.88, 0.05)], EDGE, s)
    layer(img, size, size, [rounded_rect(0.04 + px, 0.10 + px, 0.44 - px, 0.26, 0.035)], BACK, s)
    layer(img, size, size, [rounded_rect(0.04 + px, 0.16 + px, 0.96 - px, 0.88 - px, 0.045)], BACK, s)
    layer(img, size, size, [rounded_rect(0.04, 0.29, 0.96, 0.90, 0.05)], EDGE, s)
    layer(img, size, size, [rounded_rect(0.04 + px, 0.29 + px, 0.96 - px, 0.90 - px, 0.045)], None, s,
          gradient=(FRONT_TOP, FRONT_BOTTOM, 0.29, 0.90))
    # Bird on the front panel, simpler as the icon shrinks.
    if STYLE == "profile":
        sets = [(48, PROFILE_FULL, 0.62), (32, PROFILE_PLAIN, 0.66), (0, PROFILE_TINY, 0.74)]
        birds = next(place(b, 0.5, 0.59, k) for (m, b, k) in sets if size >= m)
    elif size >= 48:
        birds = place(BIRD_FULL, 0.5, 0.60, 0.58)
    elif size >= 32:
        birds = place(BIRD_PLAIN, 0.5, 0.60, 0.62)
    else:
        birds = place(WINGS_ONLY, 0.5, 0.60, 0.70)
    layer(img, size, size, birds, BIRD, s)
    return img


def png(img):
    h, w = len(img), len(img[0])
    raw = b''.join(b'\0' + bytes(min(255, int(round(c))) for p in row for c in (p[0], p[1], p[2], p[3] * 255))
                   for row in img)

    def chunk(k, d):
        return struct.pack('>I', len(d)) + k + d + struct.pack('>I', zlib.crc32(k + d) & 0xffffffff)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))


def sheet(icons):
    """Two rows (light, dark): 256, 48, 32, 16 at real size, then 32 and 16 enlarged 6x."""
    big = 6
    tiles = [icons[256], icons[48], icons[32], icons[16], scale(icons[32], big), scale(icons[16], big)]
    gap = 40
    width = sum(len(t[0]) for t in tiles) + gap * (len(tiles) + 1)
    row_h = 256 + 2 * gap
    out = []
    for bg in [(243, 243, 243), (32, 32, 32)]:
        rows = [[(bg[0], bg[1], bg[2], 1.0)] * width for _ in range(row_h)]
        x = gap
        for t in tiles:
            th, tw = len(t), len(t[0])
            y0 = (row_h - th) // 2
            for y in range(th):
                for i in range(tw):
                    r, g, b, a = t[y][i]
                    br = rows[y0 + y][x + i]
                    rows[y0 + y][x + i] = (r * a + br[0] * (1 - a), g * a + br[1] * (1 - a), b * a + br[2] * (1 - a), 1.0)
            x += tw + gap
        out += rows
    return out


def scale(img, k):
    return [[p for p in row for _ in range(k)] for row in img for _ in range(k)]


def ico(icons):
    """ICO with 32-bit BMP images (16-48 px) and a PNG image (256 px)."""
    images = []
    for n in (16, 32, 48, 256):
        img = icons[n]
        if n == 256:
            images.append((n, png(img)))
            continue
        header = struct.pack('<IiiHHIIiiII', 40, n, n * 2, 1, 32, 0, 0, 0, 0, 0, 0)
        pixels = b''.join(bytes(min(255, int(round(c))) for c in (p[2], p[1], p[0], p[3] * 255))
                          for row in reversed(img) for p in row)
        mask = b'\0' * (((n + 31) // 32) * 4 * n)
        images.append((n, header + pixels + mask))
    out = struct.pack('<HHH', 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for n, data in images:
        out += struct.pack('<BBBBHHII', n % 256, n % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return out + b''.join(d for _, d in images)


if __name__ == '__main__':
    icons = {n: icon(n) for n in (256, 48, 32, 16)}
    open(sys.argv[1], 'wb').write(ico(icons))
    if len(sys.argv) > 2:
        open(sys.argv[2], 'wb').write(png(sheet(icons)))
    print('ok')
