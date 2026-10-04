"""Expand tools/icons.txt into assets/icons/*.svg.

Each line is `name|body`; a leading `f ` draws the body filled rather than
stroked. Every icon is a 16-unit square drawn in black, which gpui treats as
a mask and tints with the element's text colour at render time.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STROKE = (
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" '
    'stroke="black" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">'
)
FILL = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="black" stroke="none">'

out = ROOT / "assets" / "icons"
out.mkdir(parents=True, exist_ok=True)
for line in (ROOT / "tools" / "icons.txt").read_text(encoding="utf-8").splitlines():
    if not line.strip() or line.startswith("#"):
        continue
    head, body = line.split("|", 1)
    filled = head.startswith("f ")
    name = head[2:] if filled else head
    (out / f"{name}.svg").write_text((FILL if filled else STROKE) + body + "</svg>\n", encoding="utf-8")
