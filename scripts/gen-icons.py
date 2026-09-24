#!/usr/bin/env python3
"""Generates Gitree's symbolic toolbar icons.

GTK recolours symbolic icons by forcing `fill` on every shape, so strokes
and `fill="none"` don't work. Icons are described with stroke-like
primitives here and converted to filled polygons / rings.
"""
import math
import os

OUT = os.path.join(os.path.dirname(__file__), "..", "data", "icons", "scalable", "actions")
W = 1.6  # default stroke width


def seg(x1, y1, x2, y2, w=W):
    dx, dy = x2 - x1, y2 - y1
    L = math.hypot(dx, dy) or 1
    nx, ny = -dy / L * w / 2, dx / L * w / 2
    pts = [(x1 + nx, y1 + ny), (x2 + nx, y2 + ny), (x2 - nx, y2 - ny), (x1 - nx, y1 - ny)]
    d = "M" + " L".join(f"{x:.2f},{y:.2f}" for x, y in pts) + " Z"
    caps = [dot(x1, y1, w / 2), dot(x2, y2, w / 2)]
    return [f'<path d="{d}"/>'] + caps


def poly(points, w=W):
    out = []
    for (a, b) in zip(points, points[1:]):
        out += seg(a[0], a[1], b[0], b[1], w)
    return out


def bezier(p0, p1, p2, p3, w=W, n=16):
    pts = []
    for i in range(n + 1):
        t = i / n
        mt = 1 - t
        x = mt**3 * p0[0] + 3 * mt**2 * t * p1[0] + 3 * mt * t**2 * p2[0] + t**3 * p3[0]
        y = mt**3 * p0[1] + 3 * mt**2 * t * p1[1] + 3 * mt * t**2 * p2[1] + t**3 * p3[1]
        pts.append((x, y))
    return poly(pts, w)


def dot(cx, cy, r):
    return f'<circle cx="{cx:.2f}" cy="{cy:.2f}" r="{r:.2f}"/>'


def ring(cx, cy, r, w=W):
    ro, ri = r + w / 2, r - w / 2
    d = (
        f"M{cx - ro:.2f},{cy:.2f} a{ro:.2f},{ro:.2f} 0 1,0 {2 * ro:.2f},0 a{ro:.2f},{ro:.2f} 0 1,0 {-2 * ro:.2f},0 Z "
        f"M{cx - ri:.2f},{cy:.2f} a{ri:.2f},{ri:.2f} 0 1,1 {2 * ri:.2f},0 a{ri:.2f},{ri:.2f} 0 1,1 {-2 * ri:.2f},0 Z"
    )
    return [f'<path fill-rule="evenodd" d="{d}"/>']


def rect_outline(x, y, w, h, sw=W):
    return poly([(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)], sw)


def rrect(x, y, w, h, r=1.5, sw=W):
    """Rounded rectangle outline as an even-odd path."""
    def rr(x, y, w, h, r):
        return (
            f"M{x + r:.2f},{y:.2f} H{x + w - r:.2f} A{r:.2f},{r:.2f} 0 0 1 {x + w:.2f},{y + r:.2f} "
            f"V{y + h - r:.2f} A{r:.2f},{r:.2f} 0 0 1 {x + w - r:.2f},{y + h:.2f} H{x + r:.2f} "
            f"A{r:.2f},{r:.2f} 0 0 1 {x:.2f},{y + h - r:.2f} V{y + r:.2f} A{r:.2f},{r:.2f} 0 0 1 {x + r:.2f},{y:.2f} Z"
        )
    h2 = sw / 2
    outer = rr(x - h2, y - h2, w + sw, h + sw, r + h2)
    inner = rr(x + h2, y + h2, w - sw, h - sw, max(r - h2, 0.1))
    return [f'<path fill-rule="evenodd" d="{outer} {inner}"/>']


ICONS = {
    "commit": ring(8, 8, 3, 2) + seg(8, 1, 8, 4.5, 2) + seg(8, 11.5, 8, 15, 2),
    "pull": seg(8, 1.5, 8, 10.5, 2) + poly([(3.8, 6.5), (8, 10.7), (12.2, 6.5)], 2) + seg(2, 14, 14, 14, 2),
    "push": seg(8, 14.5, 8, 5.5, 2) + poly([(3.8, 9.5), (8, 5.3), (12.2, 9.5)], 2) + seg(2, 2, 14, 2, 2),
    "fetch": seg(8, 1.5, 8, 3, 2) + seg(8, 5, 8, 6.5, 2) + seg(8, 8.5, 8, 10.5, 2)
    + poly([(4, 7), (8, 11), (12, 7)], 2) + seg(2, 14, 14, 14, 2),
    "branch": ring(4, 3, 1.8) + ring(4, 13, 1.8) + ring(12, 4.5, 1.8) + seg(4, 4.8, 4, 11.2)
    + bezier((12, 6.3), (12, 9.5), (4, 8.5), (4, 11.2)),
    "merge": ring(4, 3, 1.8) + ring(4, 13, 1.8) + ring(12, 11.5, 1.8) + seg(4, 4.8, 4, 11.2)
    + bezier((4, 4.8), (4, 8), (12, 7), (12, 9.7)),
    "stash": rect_outline(2.5, 6, 11, 7.5) + seg(1.5, 3, 14.5, 3) + seg(6, 9, 10, 9),
    "discard": seg(3.5, 3.5, 12.5, 12.5, 2) + seg(12.5, 3.5, 3.5, 12.5, 2),
    "tag": poly([(2, 2.5), (7, 2.5), (14, 9.5), (9, 14.5), (2, 7.5), (2, 2.5)]) + [dot(5, 5.5, 1.3)],
    "flow": ring(3, 8, 1.8) + ring(13, 3, 1.8) + ring(13, 13, 1.8) + seg(4.8, 8, 8, 8)
    + bezier((8, 8), (10, 8), (10, 3), (11.2, 3)) + bezier((8, 8), (10, 8), (10, 13), (11.2, 13)),
    "terminal": rrect(1.5, 2.5, 13, 11, 1.5) + poly([(4.2, 6), (6.7, 8), (4.2, 10)]) + seg(8.5, 10.5, 12, 10.5),
    "remote": ring(8, 8, 6) + seg(2, 8, 14, 8, 1.3) + bezier((8, 2), (5, 4.5), (5, 11.5), (8, 14), 1.3)
    + bezier((8, 2), (11, 4.5), (11, 11.5), (8, 14), 1.3),
    "history": ring(8, 8, 6) + poly([(8, 4.5), (8, 8), (10.5, 10)]),
    "filestatus": poly([(3, 1.8), (9.5, 1.8), (13, 5.3), (13, 14.2), (3, 14.2), (3, 1.8)])
    + seg(6, 9, 10, 9) + seg(8, 7, 8, 11),
    "repo": poly([(3, 13.5), (3, 2.5), (4, 1.5), (13, 1.5), (13, 12.5), (4, 12.5), (3, 13.5), (4, 14.5), (13, 14.5)], 1.5)
    + seg(6, 4.5, 10, 4.5, 1.5),
    "lfs": rrect(1.5, 3.5, 13, 9, 1.5) + seg(4, 6, 4, 10, 1.3) + poly([(6.5, 10), (6.5, 6), (8.5, 6)], 1.3)
    + seg(6.5, 8, 8, 8, 1.3) + poly([(12.5, 6), (10.5, 6), (10.5, 8), (12.5, 8), (12.5, 10), (10.5, 10)], 1.3),
    "submodule": rrect(1.5, 1.5, 13, 13, 2) + rrect(5, 5, 6, 6, 1),
    "cherry": ring(4.5, 11.5, 2.5) + ring(11.5, 11.5, 2.5) + bezier((4.5, 9), (6, 5), (8, 3), (10, 1.5), 1.4)
    + seg(11.5, 9, 10, 1.5, 1.4),
}

for name, shapes in ICONS.items():
    body = "".join(shapes)
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">'
        f'<g fill="#2e3436">{body}</g></svg>\n'
    )
    with open(os.path.join(OUT, f"gitree-{name}-symbolic.svg"), "w") as f:
        f.write(svg)
print(f"wrote {len(ICONS)} icons")
