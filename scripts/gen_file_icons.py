#!/usr/bin/env python3
"""Generates the full-color file-tree icons in assets/file-icons/.

Each icon is a standalone 16x16-viewBox SVG. They are drawn with
width/height 14 so GPUI's 2x SVG rasterizer produces exactly 28 device
pixels: pixel-perfect at the 14px size used by the tree, tabs and lists.
No <text>: monograms are stroked paths, so no font lookup at render time.

Run: python3 scripts/gen_file_icons.py
"""

from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "assets" / "file-icons"

# ---------------------------------------------------------------- helpers

def svg(body, defs=""):
    d = f"<defs>{defs}</defs>" if defs else ""
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 16 16">'
        f"{d}{body}</svg>\n"
    )


def grad(gid, top, bottom):
    return (
        f'<linearGradient id="{gid}" x1="0" y1="0" x2="0" y2="1">'
        f'<stop offset="0" stop-color="{top}"/><stop offset="1" stop-color="{bottom}"/>'
        "</linearGradient>"
    )


def tile(top, bottom, glyph):
    """Rounded square with a soft vertical gradient and a glyph on top."""
    body = (
        '<rect x="1.5" y="1.5" width="13" height="13" rx="3" fill="url(#t)"/>'
        '<rect x="1.5" y="1.5" width="13" height="13" rx="3" fill="none" stroke="#000" stroke-opacity=".18" stroke-width=".6"/>'
        + glyph
    )
    return svg(body, grad("t", top, bottom))


DOC_PATH = "M4.5 1.5H9.6L12.5 4.4V13.5Q12.5 14.5 11.5 14.5H4.5Q3.5 14.5 3.5 13.5V2.5Q3.5 1.5 4.5 1.5Z"


def doc(glyph=""):
    """White page with a folded corner (Xcode style) and an optional glyph."""
    body = (
        f'<path d="{DOC_PATH}" fill="#E3E4E8"/>'
        '<path d="M9.6 1.5V3.9Q9.6 4.4 10.1 4.4H12.5Z" fill="#A7AAB3"/>'
        + glyph
    )
    return svg(body)


def stroke(d, color="#fff", width=1.4, extra=""):
    return (
        f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{width}" '
        f'stroke-linecap="round" stroke-linejoin="round"{extra}/>'
    )


# Stroke letters in a 4x6 box (y down).
LETTERS = {
    "J": "M3 0V4.2Q3 6 1.5 6Q.4 6 0 5",
    "S": "M3.8 .9Q3.2 0 2 0Q.2 0 .2 1.5Q.2 2.8 2 3Q3.8 3.2 3.8 4.5Q3.8 6 2 6Q.7 6 0 5",
    "T": "M0 0H4M2 0V6",
    "G": "M3.8 1Q3.2 0 2 0Q0 0 0 3Q0 6 2 6Q3.8 6 3.8 3.6H2.2",
    "O": "M2 0Q4 0 4 3Q4 6 2 6Q0 6 0 3Q0 0 2 0Z",
    "C": "M3.8 1Q3.2 0 2 0Q0 0 0 3Q0 6 2 6Q3.2 6 3.8 5",
    "R": "M0 6V0H2.3Q4 0 4 1.7Q4 3.4 2.3 3.4H0M2.2 3.4L4 6",
    "H": "M0 0V6M4 0V6M0 3H4",
    "M": "M0 6V0L2 3.5L4 0V6",
    "Y": "M0 0L2 3L4 0M2 3V6",
    "E": "M4 0H0V6H4M0 3H3",
    "+": "M2 1.2V4.8M.2 3H3.8",
}


def mono(text, color="#fff"):
    """Centered stroked monogram of 1-3 characters."""
    n = len(text)
    scale = {1: 1.25, 2: .95, 3: .8}[n]
    gap = {1: 0, 2: 1.4, 3: .9}[n]
    widths = [4 if ch != "+" else 4 for ch in text]
    total = sum(w * scale for w in widths) + gap * (n - 1)
    x = 8 - total / 2
    y = 8 - 3 * scale
    out = []
    for ch, w in zip(text, widths):
        out.append(
            f'<g transform="translate({x:.2f} {y:.2f}) scale({scale})">'
            + stroke(LETTERS[ch], color, round(1.55 / scale, 2))
            + "</g>"
        )
        x += w * scale + gap
    return "".join(out)


# ---------------------------------------------------------------- files

FILES = {}

# Swift: orange tile with a simplified swift bird.
FILES["swift"] = tile(
    "#FA8B43", "#EE4F33",
    '<path fill="#fff" d="M3.6 9.5C5.5 11.3 8.6 11.9 10.6 10.6C11.3 11 11.8 11.5 12.1 12.1'
    "C12.5 10.6 12.2 9.1 11.3 7.8C10.6 5.9 9.3 4.6 7.6 3.8C8.8 5 9.6 6.4 9.7 7.9"
    "C8 6.9 6.1 5.4 4.8 4.1C5.8 5.6 7 6.9 8.3 8C6.8 7.2 5.4 6.2 4.2 5.1"
    'C5.6 7.2 7.6 9 9.6 9.9C7.8 10.6 5.7 10.4 3.6 9.5Z"/>',
)

# Kotlin: the K-wedge mark itself, purple-magenta-coral gradient.
FILES["kotlin"] = svg(
    '<path d="M3 2.5Q2.5 2.5 2.5 3V13Q2.5 13.5 3 13.5H13.5L8 8L13.5 2.5Z" fill="url(#k)"/>',
    '<linearGradient id="k" x1="0" y1="1" x2="1" y2="0">'
    '<stop offset="0" stop-color="#E44857"/><stop offset=".45" stop-color="#C711E1"/>'
    '<stop offset="1" stop-color="#7F52FF"/></linearGradient>',
)

# Java: coffee cup on a warm red tile.
FILES["java"] = tile(
    "#F0743A", "#C9372C",
    stroke("M4.6 8H10.4V9.6Q10.4 12 8 12H7Q4.6 12 4.6 9.6Z", width=1.2)
    + stroke("M10.4 8.6H11Q12 8.6 12 9.6Q12 10.6 11 10.6H10.3", width=1.1)
    + stroke("M6.6 4Q5.9 4.9 6.6 5.9M8.6 4Q7.9 4.9 8.6 5.9", width=1),
)

FILES["gradle"] = tile("#3BB3A9", "#1D7F7A", mono("G"))
FILES["rust"] = svg(
    '<circle cx="8" cy="8" r="5.1" fill="none" stroke="#DEA584" stroke-width="1.7" stroke-dasharray="1.15 1.15"/>'
    '<circle cx="8" cy="8" r="4.7" fill="#DEA584"/>'
    '<g transform="translate(6.1 5.2) scale(.95)">'
    + stroke(LETTERS["R"], "#3A2418", 1.6)
    + "</g>"
)
FILES["go"] = tile("#2EC6EE", "#0094BD", mono("GO"))
FILES["javascript"] = tile("#F7E15A", "#E7C51E", mono("JS", "#2B2B2B"))
FILES["typescript"] = tile("#4A90E0", "#2363B4", mono("TS"))

# React (tsx/jsx): atom, no tile.
FILES["react"] = svg(
    "".join(
        f'<ellipse cx="8" cy="8" rx="6.2" ry="2.4" fill="none" stroke="#61DAFB" stroke-width="1" transform="rotate({a} 8 8)"/>'
        for a in (0, 60, 120)
    )
    + '<circle cx="8" cy="8" r="1.4" fill="#61DAFB"/>'
)

# Python: two interlocking snakes, blue and yellow.
PY = (
    "M8 1.8C5.6 1.8 5.7 2.9 5.7 3.9V5.3H8.3V5.9H4.3C3 5.9 2 6.9 2 8.5C2 10.1 3 11 4.2 11H5.2"
    "V9.6C5.2 8.6 6 7.8 7 7.8H9.4C10.3 7.8 10.8 7.2 10.8 6.4V3.9C10.8 2.9 10 1.8 8 1.8Z"
)
FILES["python"] = svg(
    f'<path d="{PY}" fill="#4B8BBE"/>'
    f'<path d="{PY}" fill="#FFD43B" transform="rotate(180 8 8)"/>'
    '<circle cx="6.9" cy="3.6" r=".6" fill="#fff"/><circle cx="9.1" cy="12.4" r=".6" fill="#fff"/>'
)

# Ruby: faceted red gem.
FILES["ruby"] = svg(
    '<path d="M5 2.8H11L14 6.2L8 13.6L2 6.2Z" fill="url(#r)"/>'
    + stroke("M2.4 6.2H13.6M5 2.8L6.4 6.2L8 13.2L9.6 6.2L11 2.8", "#fff", .7, ' stroke-opacity=".5"'),
    grad("r", "#F0574B", "#B3201A"),
)

FILES["c"] = tile("#7C8CD9", "#4B5DB8", mono("C"))
FILES["cpp"] = tile("#3F86CC", "#00599C", mono("C++"))
FILES["header"] = tile("#A991DB", "#7356B8", mono("H"))
FILES["css"] = tile("#8E63D6", "#5E36A8", stroke("M6.4 4L5.6 12M10.4 4L9.6 12M4.2 6.6H11.8M4 9.4H11.6", width=1.3))
FILES["html"] = tile("#F2693D", "#D13F17", stroke("M6.2 5.2L3.8 8L6.2 10.8M9.8 5.2L12.2 8L9.8 10.8", width=1.5))
FILES["shell"] = tile("#3E4349", "#24272B", stroke("M4.4 5.4L7 8L4.4 10.6M8.2 10.8H11.6", "#8AE234", 1.5))
FILES["markdown"] = tile(
    "#5C9BD6", "#376FAA",
    stroke("M3.6 10.6V5.4L5.6 8L7.6 5.4V10.6", width=1.35)
    + stroke("M11 5.4V10.2M9.3 8.6L11 10.4L12.7 8.6", width=1.35),
)

# JSON: plain amber braces.
FILES["json"] = svg(
    stroke("M6 2.6Q4.4 2.6 4.4 4.2V6.4Q4.4 8 3 8Q4.4 8 4.4 9.6V11.8Q4.4 13.4 6 13.4", "#F0B93A", 1.5)
    + stroke("M10 2.6Q11.6 2.6 11.6 4.2V6.4Q11.6 8 13 8Q11.6 8 11.6 9.6V11.8Q11.6 13.4 10 13.4", "#F0B93A", 1.5)
    + '<circle cx="8" cy="8" r="1" fill="#F0B93A"/>'
)

# Document family.
FILES["yaml"] = doc(stroke("M5.3 7H8.2M6.6 9.2H10.6M6.6 11.4H9.6", "#D9453F", 1.2))
FILES["toml"] = doc(
    stroke("M6 6.4H5.2V8.4H6M9.6 6.4H10.4V8.4H9.6", "#9C4A26", 1)
    + stroke("M5.3 10.4H10.6M5.3 12.2H8.8", "#9C4A26", 1)
)
FILES["xml"] = doc(stroke("M6.4 7.4L4.9 9.6L6.4 11.8M9.6 7.4L11.1 9.6L9.6 11.8", "#E3772E", 1.25))
FILES["plist"] = doc(
    '<circle cx="5.6" cy="7" r=".7" fill="#5C6E85"/><circle cx="5.6" cy="9.4" r=".7" fill="#5C6E85"/>'
    '<circle cx="5.6" cy="11.8" r=".7" fill="#5C6E85"/>'
    + stroke("M7.2 7H10.6M7.2 9.4H10.6M7.2 11.8H9.6", "#5C6E85", 1)
)
FILES["text"] = doc(stroke("M5.3 6.6H10.6M5.3 8.6H10.6M5.3 10.6H10.6M5.3 12.4H8.6", "#8E9099", .9))
FILES["file"] = doc()
FILES["env"] = doc(
    '<circle cx="6.4" cy="9.6" r="1.6" fill="none" stroke="#D9A33B" stroke-width="1.1"/>'
    + stroke("M8 9.6H11M10.2 9.6V11M11 9.6V10.6", "#D9A33B", 1.1)
)
FILES["license"] = doc(
    '<path d="M6.6 10.6L6 13.6L7.2 12.9L8 13.8L8.8 12.9L10 13.6L9.4 10.6Z" fill="#B8862A"/>'
    '<circle cx="8" cy="9" r="2.3" fill="#E0AE3C"/><circle cx="8" cy="9" r="1.3" fill="none" stroke="#fff" stroke-opacity=".7" stroke-width=".6"/>'
)
FILES["readme"] = doc(
    '<circle cx="8" cy="9.6" r="3" fill="#3B82E0"/>'
    + stroke("M8 9.1V11.2", "#fff", 1.1)
    + '<circle cx="8" cy="7.9" r=".65" fill="#fff"/>'
)

FILES["lock"] = svg(
    stroke("M5.2 7V5.2Q5.2 2.4 8 2.4Q10.8 2.4 10.8 5.2V7", "#A7AAB3", 1.5)
    + '<rect x="3.6" y="7" width="8.8" height="6.8" rx="1.6" fill="url(#l)"/>'
    + '<circle cx="8" cy="9.9" r=".9" fill="#5E4710"/>' + stroke("M8 10.4V11.8", "#5E4710", 1),
    grad("l", "#EBC24F", "#C9971F"),
)

# Git: the rotated-square mark with a branch.
FILES["git"] = svg(
    '<rect x="3" y="3" width="10" height="10" rx="2" fill="#F05033" transform="rotate(45 8 8)"/>'
    + stroke("M6.2 5.2L10 9M6.2 5.2V10.8", "#fff", 1.1)
    + '<circle cx="6.2" cy="5.2" r="1" fill="#fff"/><circle cx="6.2" cy="10.8" r="1" fill="#fff"/>'
    '<circle cx="10" cy="9" r="1" fill="#fff"/>'
)

FILES["docker"] = tile(
    "#3BA6F2", "#1D7BC9",
    "".join(
        f'<rect x="{x}" y="{y}" width="2" height="1.8" rx=".3" fill="#fff"/>'
        for x, y in [(3.4, 7.8), (5.8, 7.8), (8.2, 7.8), (5.8, 5.6), (8.2, 5.6), (8.2, 3.4)]
    )
    + stroke("M3 10.3H12.2Q12.6 9 13.4 9", width=1.1)
    + stroke("M3.2 10.4Q4 12.6 7.4 12.6Q10.6 12.6 12 10.4", width=1),
)

FILES["image"] = svg(
    '<rect x="1.5" y="2.5" width="13" height="11" rx="2.2" fill="url(#i)"/>'
    '<path d="M1.5 11.5L5.6 7.4L8.4 10.2L10.2 8.4L14.5 12.4V11.3Q14.5 13.5 12.3 13.5H3.7Q1.5 13.5 1.5 11.3Z" fill="#E8F6F2"/>'
    '<circle cx="10.6" cy="5.6" r="1.4" fill="#FFD55A"/>',
    grad("i", "#37B39C", "#1F7F78"),
)

HAMMER = (
    stroke("M5 11.6L9 7.6", width=1.5)
    + '<path d="M7.6 4.6L10.2 3.6L12.6 6L11.6 8.6L10.4 7.4L9.6 8.2L8 6.6L8.8 5.8Z" fill="#fff"/>'
)
FILES["xcodeproj"] = tile("#4AA3FF", "#1767D6", HAMMER)
FILES["xcworkspace"] = svg(
    '<rect x="4" y="1" width="11" height="11" rx="2.6" fill="#8FB4E8"/>'
    '<rect x="1.5" y="3.5" width="11.5" height="11.5" rx="2.8" fill="url(#t)"/>'
    '<g transform="translate(-.8 1.4)">' + HAMMER + "</g>",
    grad("t", "#4AA3FF", "#1767D6"),
)

# ---------------------------------------------------------------- folders

BACK = "M1.5 4Q1.5 3 2.5 3H5.8Q6.3 3 6.6 3.4L7.4 4.4H13.5Q14.5 4.4 14.5 5.4V12Q14.5 13 13.5 13H2.5Q1.5 13 1.5 12Z"
FRONT_CLOSED = "M1.5 6.6Q1.5 5.8 2.3 5.8H13.7Q14.5 5.8 14.5 6.6V12Q14.5 13 13.5 13H2.5Q1.5 13 1.5 12Z"
FRONT_OPEN = "M3.6 7.1Q3.8 6.6 4.4 6.6H14.6Q15.4 6.6 15.2 7.3L13.7 12.3Q13.5 13 12.8 13H2.2Q1.4 13 1.6 12.3Z"

# Glyphs centered around (8, 9.6) on the front panel.
FOLDER_GLYPHS = {
    "src": stroke("M6.6 8L5.2 9.5L6.6 11M9.4 8L10.8 9.5L9.4 11", "#fff", 1.1),
    "assets": '<path d="M5 11.4L7 9.2L8.4 10.4L9.4 9.6L11 11.4Z" fill="#fff"/><circle cx="9.8" cy="8.1" r=".8" fill="#fff"/>',
    "test": stroke("M5.5 9.6L7.3 11.2L10.6 7.9", "#fff", 1.3),
    "github": stroke("M6.4 8V11.2M6.4 9.9Q9.6 9.9 9.6 8", "#fff", 1)
    + '<circle cx="6.4" cy="7.9" r=".8" fill="#fff"/><circle cx="6.4" cy="11.3" r=".8" fill="#fff"/><circle cx="9.6" cy="7.9" r=".8" fill="#fff"/>',
    "build": '<path d="M8 7.4L10.2 8.5V10.9L8 12L5.8 10.9V8.5Z" fill="none" stroke="#fff" stroke-width="1" stroke-linejoin="round"/>'
    + stroke("M5.9 8.5L8 9.6L10.1 8.5M8 9.6V11.9", "#fff", .8),
    "deps": '<rect x="5.3" y="8" width="5.4" height="3.8" rx=".5" fill="none" stroke="#fff" stroke-width="1"/>'
    + stroke("M5.3 9.3H10.7M8 8V9.3", "#fff", .9),
    "ios": '<rect x="6.3" y="7.2" width="3.4" height="5" rx=".8" fill="none" stroke="#2B2E33" stroke-width=".9"/>'
    '<circle cx="8" cy="11.2" r=".35" fill="#2B2E33"/>',
    "android": '<path d="M5.4 11.2Q5.4 8.6 8 8.6Q10.6 8.6 10.6 11.2Z" fill="#fff"/>'
    + stroke("M5.9 7.6L6.6 8.7M10.1 7.6L9.4 8.7", "#fff", .7)
    + '<circle cx="7" cy="10.1" r=".35" fill="#1F7A4A"/><circle cx="9" cy="10.1" r=".35" fill="#1F7A4A"/>',
    "gradle": '<g transform="translate(6.5 7.6) scale(.7)">' + stroke(LETTERS["G"], "#fff", 1.5) + "</g>",
}

FOLDERS = {
    # name: (back, front)
    "folder": ("#3C86D8", "#62AAF2"),
    "src": ("#3C86D8", "#62AAF2"),
    "assets": ("#C9457E", "#EC72A6"),
    "test": ("#2F9E5B", "#4CC27A"),
    "github": ("#4F5560", "#757C88"),
    "build": ("#5E5E63", "#86868C"),
    "deps": ("#8B6B2E", "#B78E40"),
    "ios": ("#8A919D", "#C3C8D1"),
    "android": ("#26A35E", "#3DDC84"),
    "gradle": ("#1E7F86", "#2CA6AD"),
}


def folder(back, front, glyph, open_):
    shape = FRONT_OPEN if open_ else FRONT_CLOSED
    g = glyph
    if open_ and glyph:
        g = f'<g transform="translate(.8 .2)">{glyph}</g>'
    return svg(f'<path d="{BACK}" fill="{back}"/><path d="{shape}" fill="{front}"/>' + g)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for old in OUT.glob("*.svg"):
        old.unlink()
    for name, body in FILES.items():
        (OUT / f"{name}.svg").write_text(body)
    for name, (back, front) in FOLDERS.items():
        glyph = FOLDER_GLYPHS.get(name, "")
        base = "folder" if name == "folder" else f"folder-{name}"
        (OUT / f"{base}.svg").write_text(folder(back, front, glyph, False))
        (OUT / f"{base}-open.svg").write_text(folder(back, front, glyph, True))
    print(f"wrote {len(list(OUT.glob('*.svg')))} icons to {OUT}")


if __name__ == "__main__":
    main()
