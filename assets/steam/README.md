# Steam library artwork

Images of the Steam library entry that `urakata steam-entry --add` makes.
They are built into `urakata.exe` (`crates/urakata/src/steam_art.rs`).

| File | Steam name | Size |
| --- | --- | --- |
| `capsule.png` | `<appid>p.png`, upright capsule | 600 x 900 |
| `wide.png` | `<appid>.png`, wide capsule | 920 x 430 |
| `hero.png` | `<appid>_hero.png`, top of the game page | 1920 x 620 |
| `logo.png` | `<appid>_logo.png`, on the hero, transparent | 1015 x 356 |

The icon is `assets/logo/urakata.ico`, written as `<appid>_icon.ico`.

The SVG files are generated. To change the artwork, edit `gen.py`, then
from the repo root:

    python3 assets/steam/gen.py      # needs fontTools (pip install fonttools)
    node assets/steam/render.js      # needs Playwright with Chromium

`gen.py` takes the mark from `assets/logo/urakata.svg` and the wordmark
from Inter SemiBold (`crates/urakata-app/ui/fonts`), as outlines.
