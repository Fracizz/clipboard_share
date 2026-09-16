# Application icon

`clipboardshare-matte.png` is the current master with a transparent background,
generated with the built-in image generation tool. It combines the approved
clipboard shape with the supplied reference's opaque blue gradients: three
layers, a clipboard clip, and two inset content lines, without glass or arrows.
The exact edit prompt is saved in `clipboardshare-matte-prompt.txt`.

Regenerate the frontend PNG, desktop PNG/ICO/ICNS files, compact tray icons,
and light/dark preview with:

```sh
uv run tools/generate_icons.py
```

`clipboardshare-tray.svg` is the editable compact version for small sizes.
`clipboardshare-icon.svg` is the earlier vector design, retained as a source
archive; it is no longer used to generate application icons.
`clipboardshare-glass.png` preserves the previous glass variant for reference.
