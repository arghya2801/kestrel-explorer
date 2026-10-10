"""Writes a simple folder icon (16, 32, 48, 256 px, 32-bit) as an .ico file. No dependencies."""
import struct
import sys
import zlib


def folder(size):
    """RGBA rows, top to bottom: a yellow folder with a darker tab and a blue lightning stripe."""
    px = [[(0, 0, 0, 0)] * size for _ in range(size)]
    s = size / 16.0

    def rect(x0, y0, x1, y1, color):
        for y in range(int(y0 * s), int(y1 * s)):
            for x in range(int(x0 * s), int(x1 * s)):
                if 0 <= x < size and 0 <= y < size:
                    px[y][x] = color

    edge = (176, 122, 18, 255)
    back = (232, 168, 36, 255)
    front_top = (255, 214, 92, 255)
    front_bottom = (246, 188, 56, 255)
    rect(1, 2, 7, 4, edge)          # tab outline
    rect(1.5, 2.5, 6.5, 4, back)    # tab
    rect(1, 3.5, 15, 14, edge)      # body outline
    rect(1.5, 4, 14.5, 13.5, back)  # back panel
    # front panel with a vertical gradient
    y0, y1 = int(6 * s), int(13.5 * s)
    for y in range(y0, y1):
        t = (y - y0) / max(1, y1 - y0 - 1)
        c = tuple(int(front_top[i] * (1 - t) + front_bottom[i] * t) for i in range(3)) + (255,)
        for x in range(int(1.5 * s), int(14.5 * s)):
            px[y][x] = c
    rect(1, 5.5, 15, 6, edge)       # front panel top edge
    # speed stripe: the "fast" part
    stripe = (40, 110, 220, 255)
    for i in range(int(5 * s)):
        y = int(12 * s) - i
        x = int(6 * s) + i
        for w in range(max(1, int(1.2 * s))):
            if 0 <= y < size and 0 <= x + w < size:
                px[y][x + w] = stripe
    return px


def bmp_entry(size):
    rows = folder(size)
    header = struct.pack('<IiiHHIIiiII', 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = b''.join(bytes((b, g, r, a)) for row in reversed(rows) for (r, g, b, a) in row)
    mask_row = ((size + 31) // 32) * 4
    return header + pixels + b'\0' * (mask_row * size)


def png_entry(size):
    rows = folder(size)
    raw = b''.join(b'\0' + bytes(c for p in row for c in p) for row in rows)

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff)

    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))


sizes = [16, 32, 48, 256]
images = [png_entry(s) if s == 256 else bmp_entry(s) for s in sizes]
out = struct.pack('<HHH', 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
for s, img in zip(sizes, images):
    out += struct.pack('<BBBBHHII', s % 256, s % 256, 0, 0, 1, 32, len(img), offset)
    offset += len(img)
out += b''.join(images)
open(sys.argv[1], 'wb').write(out)
print(len(out), 'bytes')
