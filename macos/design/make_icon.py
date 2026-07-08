"""
Genjux-Store macOS app icon (#65).

Design: a soft indigo->violet gradient rounded-square field (the
"developer tool" palette shared by a lot of modern dev-focused macOS
apps), with a single bold glyph -- a package/box with a downward arrow
entering it, the universal "install" symbol -- rendered in white with a
subtle drop shadow for depth. No text, per Apple's Human Interface
Guidelines for app icons; a single, instantly-recognizable shape that
still reads clearly at 16x16.

Rendered at 1024x1024 (full bleed square -- macOS 11+ applies its own
rounded "squircle" mask automatically at render time), then downscaled
to every size AppIcon.appiconset's Contents.json declares.
"""
from PIL import Image, ImageDraw, ImageFilter
import math

SIZE = 1024
img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))

# --- Background: soft diagonal gradient, indigo -> violet ---
top_color = (79, 70, 229)     # #4F46E5 indigo
bottom_color = (124, 58, 237) # #7C3AED violet

bg = Image.new("RGB", (SIZE, SIZE))
for y in range(SIZE):
    t = y / (SIZE - 1)
    r = int(top_color[0] + (bottom_color[0] - top_color[0]) * t)
    g = int(top_color[1] + (bottom_color[1] - top_color[1]) * t)
    b = int(top_color[2] + (bottom_color[2] - top_color[2]) * t)
    ImageDraw.Draw(bg).line([(0, y), (SIZE, y)], fill=(r, g, b))

# Rounded-square mask so the background itself has soft corners even
# before macOS applies its own squircle mask on top (keeps a clean edge
# if ever rendered somewhere that doesn't auto-mask).
corner_radius = int(SIZE * 0.223)  # matches macOS's own icon corner ratio
mask = Image.new("L", (SIZE, SIZE), 0)
ImageDraw.Draw(mask).rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius=corner_radius, fill=255)
bg.putalpha(mask)
img = Image.alpha_composite(img, bg)

# --- Subtle top-left highlight + bottom-right shadow for depth ---
shade = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
shade_draw = ImageDraw.Draw(shade)
# Soft radial highlight, upper-left
highlight = Image.new("L", (SIZE, SIZE), 0)
hd = ImageDraw.Draw(highlight)
hd.ellipse([-SIZE * 0.3, -SIZE * 0.3, SIZE * 0.75, SIZE * 0.75], fill=70)
highlight = highlight.filter(ImageFilter.GaussianBlur(SIZE * 0.12))
white_layer = Image.new("RGBA", (SIZE, SIZE), (255, 255, 255, 255))
img = Image.composite(Image.alpha_composite(img, Image.merge("RGBA", (*white_layer.split()[:3], highlight))), img, mask)

# Soft shadow, lower-right
shadow = Image.new("L", (SIZE, SIZE), 0)
sd = ImageDraw.Draw(shadow)
sd.ellipse([SIZE * 0.35, SIZE * 0.45, SIZE * 1.3, SIZE * 1.3], fill=60)
shadow = shadow.filter(ImageFilter.GaussianBlur(SIZE * 0.12))
black_layer = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 255))
img = Image.composite(Image.alpha_composite(img, Image.merge("RGBA", (*black_layer.split()[:3], shadow))), img, mask)

# --- Foreground glyph: an open box/tray with a downward arrow entering it ---
glyph = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
gd = ImageDraw.Draw(glyph)

cx, cy = SIZE / 2, SIZE / 2
stroke = int(SIZE * 0.052)
white = (255, 255, 255, 255)
white_soft = (255, 255, 255, 235)

# Tray: an open-top trapezoid (box viewed from a slight angle), drawn as
# a wide "U" shape with straight sides flaring slightly outward.
tray_top_y = cy + SIZE * 0.06
tray_bottom_y = cy + SIZE * 0.235
tray_half_top = SIZE * 0.205
tray_half_bottom = SIZE * 0.245

tray_points_left = [
    (cx - tray_half_top, tray_top_y),
    (cx - tray_half_bottom, tray_bottom_y),
]
tray_points_right = [
    (cx + tray_half_bottom, tray_bottom_y),
    (cx + tray_half_top, tray_top_y),
]

# Bottom of tray
gd.line([tray_points_left[1], tray_points_right[0]], fill=white, width=stroke, joint="curve")
# Left side
gd.line(tray_points_left, fill=white, width=stroke, joint="curve")
# Right side
gd.line(tray_points_right, fill=white, width=stroke, joint="curve")
# Rounded caps at the open top ends
cap_r = stroke / 2
for pt in [tray_points_left[0], tray_points_right[1]]:
    gd.ellipse([pt[0] - cap_r, pt[1] - cap_r, pt[0] + cap_r, pt[1] + cap_r], fill=white)
for pt in [tray_points_left[1], tray_points_right[0]]:
    gd.ellipse([pt[0] - cap_r, pt[1] - cap_r, pt[0] + cap_r, pt[1] + cap_r], fill=white)

# Downward arrow entering the tray: a vertical shaft + arrowhead,
# positioned just above the tray opening.
shaft_top = cy - SIZE * 0.235
shaft_bottom = cy + SIZE * 0.005
shaft_width = stroke

gd.line([(cx, shaft_top), (cx, shaft_bottom)], fill=white, width=shaft_width)
# Rounded cap at shaft top
gd.ellipse(
    [cx - shaft_width / 2, shaft_top - shaft_width / 2, cx + shaft_width / 2, shaft_top + shaft_width / 2],
    fill=white,
)

arrow_half_width = SIZE * 0.11
arrow_height = SIZE * 0.105
arrow_tip_y = cy + SIZE * 0.045
arrow_points = [
    (cx, arrow_tip_y),
    (cx - arrow_half_width, arrow_tip_y - arrow_height),
    (cx - arrow_half_width * 0.38, arrow_tip_y - arrow_height),
    (cx - arrow_half_width * 0.38, shaft_top + SIZE * 0.02),
    (cx + arrow_half_width * 0.38, shaft_top + SIZE * 0.02),
    (cx + arrow_half_width * 0.38, arrow_tip_y - arrow_height),
    (cx + arrow_half_width, arrow_tip_y - arrow_height),
]
gd.polygon(arrow_points, fill=white_soft)

img = Image.alpha_composite(img, glyph)

import os

HERE = os.path.dirname(os.path.abspath(__file__))
master_path = os.path.join(HERE, "icon-master.png")
img.save(master_path)
print(f"saved master icon -> {master_path}")

# --- Regenerate every size the asset catalog's Contents.json declares ---
# (16/32/64/128/256/512/1024, matching the standard mac idiom 1x/2x set).
appiconset_dir = os.path.join(HERE, "..", "GenjuxStore", "Assets.xcassets", "AppIcon.appiconset")
sizes = [
    (16, "icon_16x16.png"),
    (32, "icon_16x16@2x.png"),
    (32, "icon_32x32.png"),
    (64, "icon_32x32@2x.png"),
    (128, "icon_128x128.png"),
    (256, "icon_128x128@2x.png"),
    (256, "icon_256x256.png"),
    (512, "icon_256x256@2x.png"),
    (512, "icon_512x512.png"),
    (1024, "icon_512x512@2x.png"),
]
for px, name in sizes:
    resized = img.resize((px, px), Image.LANCZOS)
    out_path = os.path.join(appiconset_dir, name)
    resized.save(out_path)
    print(f"  {name} ({px}x{px}) -> {out_path}")

print("Done. Run `xcodegen generate` in macos/ before building.")
