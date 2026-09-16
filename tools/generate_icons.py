# /// script
# requires-python = ">=3.10"
# dependencies = ["pillow==12.3.0"]
# ///
"""Regenerate desktop icons: uv run tools/generate_icons.py.

The matte PNG is used for the application artwork. Small Windows tray/taskbar
sizes use a separate, simplified renderer so their edges and content remain
legible after Windows scales them down.
"""

from pathlib import Path
import shutil

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets"
ICONS = ROOT / "ui/src-tauri/icons"


def render(size):
    with Image.open(ASSETS / "clipboardshare-matte.png") as image:
        return image.convert("RGBA").resize((size, size), Image.Resampling.LANCZOS)


def render_tray(size):
    """Render a high-contrast clipboard mark for small OS icon surfaces."""
    scale = 8
    canvas_size = 32 * scale
    image = Image.new("RGBA", (canvas_size, canvas_size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    def box(x1, y1, x2, y2):
        return tuple(round(value * scale) for value in (x1, y1, x2, y2))

    draw.rounded_rectangle(
        box(3, 3, 22, 26),
        radius=4 * scale,
        fill="#0756ce",
        outline="#67caff",
        width=round(1.5 * scale),
    )
    draw.rounded_rectangle(
        box(9, 7, 30, 30),
        radius=4 * scale,
        fill="#48baff",
    )
    draw.rounded_rectangle(
        box(15, 4, 25, 10),
        radius=2 * scale,
        fill="#d7f7ff",
    )
    line_width = round(2.5 * scale)
    draw.line((14 * scale, 16 * scale, 25 * scale, 16 * scale), fill="#063e89", width=line_width)
    draw.line((14 * scale, 23 * scale, 25 * scale, 23 * scale), fill="#063e89", width=line_width)

    return image.resize((size, size), Image.Resampling.LANCZOS)


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
        render(size).save(ICONS / name)
    for size in (16, 20, 24, 32, 48):
        render_tray(size).save(ICONS / f"tray-{size}.png")
    # Tauri decodes only the first ICO entry for the runtime window icon.
    # Supply a separate high-resolution image instead of enlarging that 16px entry.
    render_tray(64).save(ICONS / "window-64.png")

    frames = [
        render_tray(size) if size <= 64 else render(size)
        for size in (16, 20, 24, 32, 40, 48, 64, 128, 256)
    ]
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
            icon = render_tray(size)
            preview.paste(icon, (x, 368 - size // 2), icon)
            draw.text((x, 402), str(size), fill=color)
            x += 76
    preview.save(ASSETS / "clipboardshare-icon-preview.png")
    print("Generated desktop icons, tray icons, frontend PNG and preview.")


if __name__ == "__main__":
    main()
