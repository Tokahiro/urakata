# Command line

`urakata.exe` is in `%LOCALAPPDATA%\Programs\urakata` (the default
install folder). The setup does not add this folder to `PATH`, so run it
with its full path, for example in PowerShell:

```powershell
& "$env:LOCALAPPDATA\Programs\urakata\urakata.exe" status
```

`urakata --help` lists all commands and options.
`urakata --version` shows the installed version. The programs also carry
it in their file properties (Properties > Details in Explorer).

## status

```text
urakata status [--json]
```

Shows the SudoVDA driver, the active displays and the setup checks (the
same checks as the **Status** page of the settings window). It changes
nothing. `--json` prints the same data as JSON.

## exclusive and restore

```text
urakata exclusive [--dry-run]
urakata restore [--dry-run]
```

The watcher does this for you. Use these commands only to test or to fix
something by hand.

- `exclusive`: during a stream, turns off all displays except the
  SudoVDA ones. It saves the layout first. Without a stream it refuses.
- `restore`: turns the displays on again in the layout Windows stored for
  them, and sets HDR back to the saved state.
- `--dry-run`: asks Windows whether the change would work, but changes
  nothing.

## watch

```text
urakata watch [--install | --uninstall]
```

- `watch`: runs the watcher in this terminal. Usually it runs as
  `urakata-watch.exe` in the background instead.
- `--install`: starts the watcher at each logon of your Windows user. No
  admin needed.
- `--uninstall`: does not start the watcher at logon any more.

The watcher writes its log to `%LOCALAPPDATA%\urakata\watch.log`.

## driver

```text
urakata driver status [--json]
urakata driver install | reinstall | repair [--package <dir>] [--log <file>]
urakata driver uninstall [--keep-certificate] [--log <file>]
```

- `status`: shows the state of the SudoVDA driver.
- `install`: installs what is missing.
- `reinstall`: installs the driver again.
- `repair`: fixes a broken driver setup, for example a missing
  certificate.
- `uninstall`: removes the driver and its certificate.
  `--keep-certificate` leaves the certificate in place.
- `--package <dir>`: the folder with the driver files. The default is the
  `driver` folder next to `urakata.exe`. The files must match the hashes
  that urakata knows.
- `--log <file>`: also writes the output to this file.

All driver commands except `status` need admin rights (run the terminal
as administrator) and refuse while a stream runs. The **Displays** page
of the settings window does the same repair with one admin prompt.

## update

```text
urakata update [--check]
```

Looks for a newer release on GitHub. When there is one, it downloads the
setup, checks its SHA-256 and runs it without questions. Your settings
and the driver stay as they are. The logon start follows your last setup
choice (see [Update](install.md#update)). The **Install** button on the
**About** page of the settings window does the same.

- `--check`: only says whether a newer release exists.

It refuses while a stream runs, and when the terminal runs as
administrator.

## steam-entry

```text
urakata steam-entry [--add | --remove]
```

- Without an option: shows for each Steam account on the PC whether the
  library has the entry **urakata**.
- `--add`: adds the entry, with its icon and library artwork. If the
  entry is there already, it corrects the program path.
- `--remove`: removes the entry.

Steam must not run for `--add` and `--remove`. Exit Steam first.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | An error, or the command refused to run (for example `exclusive` without a stream, `update` during a stream, `steam-entry --add` while Steam runs, a driver change without admin rights). |
| 2 | Usage error. |
| 3 | Done, but with problems: some status parts could not be read, `restore` finished with warnings, the driver still needs a repair, or `steam-entry` failed for a Steam account. |
| 10 | `driver status`: SudoVDA is not installed. |
| 11 | `driver status`: SudoVDA needs a repair. |
