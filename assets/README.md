# Application icon

`clipboardshare-matte.png` is the current application master with a transparent
background, generated with the built-in image generation tool. It combines the
approved clipboard shape with the supplied reference's opaque blue gradients:
three layers, a clipboard clip, and two inset content lines, without glass or
arrows.
The exact edit prompt is saved in `clipboardshare-matte-prompt.txt`.

Regenerate the frontend PNG, desktop PNG/ICO/ICNS files, optimized tray icons,
and light/dark preview with:

```sh
uv run tools/generate_icons.py
```

Application PNGs and large ICO frames use the master PNG. The tray PNGs and
16–64 px ICO frames use a simplified high-contrast mark designed for small
Windows surfaces.
`window-64.png` supplies the runtime Windows window icon explicitly, bypassing
Tauri's first-ICO-frame decoding (which otherwise selects the 16 px entry).
`ui/src-tauri/build.rs` tracks the icons directory so native EXE resources are
rebuilt after changes, including incremental builds.
`clipboardshare-tray.svg` is the earlier compact design, retained for reference only.
`clipboardshare-icon.svg` is the earlier vector design, retained as a source
archive; it is no longer used to generate application icons.
`clipboardshare-glass.png` preserves the previous glass variant for reference.
