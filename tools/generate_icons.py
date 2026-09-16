# /// script
# requires-python = ">=3.10"
# dependencies = ["cairosvg==2.9.1", "pillow==12.3.0"]
# ///
"""Regenerate desktop icons: uv run tools/generate_icons.py.

CairoSVG also requires the native Cairo library. The matte PNG in assets/ is
the application master; the compact SVG is used for icons of 32 px or less.
"""

from io import BytesIO
from pathlib import Path
import shutil

import cairosvg
from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets"
ICONS = ROOT / "ui/src-tauri/icons"


def render(size, compact=False):
    if not compact:
        with Image.open(ASSETS / "clipboardshare-matte.png") as image:
            return image.convert("RGBA").resize((size, size), Image.Resampling.LANCZOS)
    source = ASSETS / "clipboardshare-tray.svg"
    # Supersampling preserves smooth curves in the small Windows icon frames.
    png = cairosvg.svg2png(url=str(source), output_width=size * 4, output_height=size * 4)
    with Image.open(BytesIO(png)) as image:
        return image.convert("RGBA").resize((size, size), Image.Resampling.LANCZOS)


def main():
    master = render(1024)
    master.save(ASSETS / "clipboardshare-icon-1024.png")
    master.save(ROOT / "ui/app-icon.png")
    shutil.copyfile(ASSETS / "clipboardshare-matte.png", ROOT / "ui/src/assets/clipboardshare.png")

    sizes = {
        "icon.png": 512, "32x32.png": 32, "64x64.png": 64,
        "128x128.png": 128, "128x128@2x.png": 256,
        "StoreLogo.png": 50,
        **{f"Square{size}x{size}Logo.png": size for size in (30, 44, 71, 89, 107, 142, 150, 284, 310)},
    }
    for name, size in sizes.items():
        render(size, compact=size <= 32).save(ICONS / name)
    for size in (16, 20, 24, 32, 48):
        render(size, compact=True).save(ICONS / f"tray-{size}.png")

    frames = [render(size, compact=size <= 32) for size in (16, 20, 24, 32, 40, 48, 64, 128, 256)]
    frames[-1].save(ICONS / "icon.ico", sizes=[im.size for im in frames], append_images=frames[:-1])
    master.save(ICONS / "icon.icns")

    # Review sheet: actual-size icons on light and dark backgrounds.
    preview = Image.new("RGB", (880, 440), "#f4f7fb")
    draw = ImageDraw.Draw(preview)
    draw.rectangle((440, 0, 879, 439), fill="#101827")
    for offset, color in ((0, "#24446b"), (440, "#cadcf2")):
        icon = render(280)
        preview.paste(icon, (offset + 80, 24), icon)
        draw.text((offset + 32, 326), "TRAY / ACTUAL SIZE", fill=color)
        x = offset + 32
        for size in (16, 20, 24, 32, 48):
            icon = render(size, compact=True)
            preview.paste(icon, (x, 368 - size // 2), icon)
            draw.text((x, 402), str(size), fill=color)
            x += 76
    preview.save(ASSETS / "clipboardshare-icon-preview.png")
    print("Generated desktop icons, tray icons, frontend PNG and preview.")


if __name__ == "__main__":
    main()
