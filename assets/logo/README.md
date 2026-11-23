# Logo

The mark is *Maku* (幕, the stage curtain): the PC display is the stage,
the curtain opens, and the light of the show comes out.

| File | Use |
| --- | --- |
| `urakata.svg` | Stream active: curtain open. Source for 40 px and larger. |
| `urakata-small.svg` | Same, simpler drawing for 32 px and smaller. |
| `urakata-idle.svg` | Idle: curtain closed, grey frame. |
| `urakata-idle-small.svg` | Idle, for 32 px and smaller. |
| `urakata.ico`, `urakata-idle.ico` | 16, 20, 24, 32, 40, 48, 64, 128, 256 px. |
| `urakata-256.png` | Active state, 256 px. |

The SVG files are the source. Colours come from the Kidō tokens in
`crates/urakata-app/ui/kido.slint` (`accent-line` `#ff00ff`,
`accent-label` `#03edf9`).

To rebuild an `.ico`: render the SVG to PNG at each size with a browser
engine (small SVG for 16 to 32 px, large SVG above), then
`convert urakata-16.png ... urakata-256.png urakata.ico` (ImageMagick).

Where the icons are used:

- `icons.rc` puts both `.ico` files into `urakata.exe`,
  `urakata-watch.exe` and `urakata-app.exe` (the build scripts of the two
  crates compile it). Number 1 is the open curtain, which Explorer shows;
  number 2 is the closed curtain.
- The tray icon of the watcher loads them by these numbers: closed curtain
  while no stream runs, open curtain while one runs.
- The installer uses `urakata.ico` (`SetupIconFile`).
- The settings window uses `crates/urakata-app/ui/icon.png`, which is
  `urakata-small.svg` at 64 px. Render it again when the logo changes.
