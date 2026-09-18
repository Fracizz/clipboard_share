# Application icon

`clipboardshare-matte.png` is the current application master with a transparent
background, generated with the built-in image generation tool. It combines the
approved clipboard shape with the supplied reference's opaque blue gradients:
three layers, a clipboard clip, and two inset content lines, without glass or
arrows.
The exact edit prompt is saved in `clipboardshare-matte-prompt.txt`.

Regenerate the frontend PNG, desktop PNG/ICO/ICNS files, tray icons,
and light/dark preview with:

```sh
uv run tools/generate_icons.py
```

All application, window, taskbar, and tray icons use this one master PNG.
Every output size is downsampled directly from the master using Lanczos;
there is no separate small-icon design. The Windows ICO contains native
16, 20, 24, 32, 40, 48, 64, 96, 128, and 256 px frames for different DPI scales.
`128x128@2x.png` supplies the 256 px runtime Windows window icon explicitly,
bypassing Tauri's first-ICO-frame decoding (which otherwise selects the 16 px
entry). The tray uses `tray-64.png` to cover high-DPI notification areas.
`ui/src-tauri/build.rs` tracks the icons directory so native EXE resources are
rebuilt after changes, including incremental builds.
`clipboardshare-tray.svg` is the earlier compact design, retained for reference only.
`clipboardshare-icon.svg` is the earlier vector design, retained as a source
archive; it is no longer used to generate application icons.
`clipboardshare-glass.png` preserves the previous glass variant for reference.
