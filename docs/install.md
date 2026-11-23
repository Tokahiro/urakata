# Install, update and uninstall

## Requirements

- Windows 10 version 2004 or newer, or Windows 11, 64-bit.
- Steam with SudoVDA support. Valve added it to the Steam client in
  April 2026.
- Admin rights once, for the driver step. The rest needs no admin.

urakata was tested on Windows 11 with an NVIDIA graphics card and with
phone and laptop clients. Windows 10 and other graphics cards were
not tested.

## Install

1. Download `urakata-setup-<version>.exe` from the
   [latest release](https://github.com/Tokahiro/urakata/releases/latest).
   To check the download, compare its SHA-256 with the
   `.sha256` file next to it:

   ```powershell
   Get-FileHash .\urakata-setup-<version>.exe -Algorithm SHA256
   ```

2. Run the setup. It installs into
   `%LOCALAPPDATA%\Programs\urakata`, for your Windows user only.
3. Keep the task **Start the urakata watcher at each logon
   (recommended)** on. Without
   it, urakata does nothing during a stream until you start the watcher.
4. On the page **SudoVDA driver**, pick what to do with the driver. The
   page shows the current driver state:

   | Driver state | Choices (the first one is the default) |
   | --- | --- |
   | Not installed | Install the bundled driver; do not install now |
   | Installed and works | Keep the installed driver; reinstall the bundled driver |
   | Needs a repair | Repair; reinstall; leave it |

   A driver change shows one Windows admin prompt for urakata. Do not
   change the driver while a stream runs.
5. Finish the setup. The watcher starts at once.

Then do the [first-run setup](setup.md#first-run).

### What the setup installs

- `urakata.exe`: the command line tool ([Command line](commands.md)).
- `urakata-watch.exe`: the watcher. It runs in the background, with an
  optional tray icon.
- `urakata-app.exe`: the settings window. The Start menu entry
  **urakata** opens it.
- `driver\`: the SudoVDA driver 1.10.9.289, as Apollo 0.4.6 ships it.
  urakata checks the files before it installs them.

The driver install also adds SudoMaker's driver certificate to the
machine's Root and TrustedPublisher certificate stores. Windows loads the
driver only with this certificate. If you already use SudoVDA with
another program (for example Apollo), keep the installed driver.

### SmartScreen

The setup and the programs have no code signature yet. Windows
SmartScreen can show "Windows protected your PC" on the first start.
Click **More info**, check that the file is the one you downloaded, then
click **Run anyway**.

## Update

- In the settings window, open **About** and press **Look for updates**.
  If a newer release exists, press **Release page** and run the new
  setup.
- Or run `urakata update` (see [Command line](commands.md#update)). It
  downloads the new setup from GitHub, checks its SHA-256 and runs it
  without questions. This path was not yet tested with a published
  release. If it fails, download and run the setup by hand.

An update keeps your settings and the driver. Do not update while a
stream runs. The logon start goes back to your choice in the last setup
run: if you changed **Start at logon** since then, check it on the
**Watcher** page.

## Uninstall

1. End any stream.
2. If you added the Steam library entry, remove it first: exit Steam,
   then run `urakata steam-entry --remove` (see
   [Command line](commands.md#steam-entry)). The uninstaller does not
   remove it.
3. Open **Settings > Apps > Installed apps** in Windows, find
   **urakata** and click **Uninstall**.
4. The uninstaller stops the watcher and removes the logon start. Then it
   asks whether to remove the SudoVDA driver and its certificate too. The
   default is **No**, because Steam and other programs use the same
   driver.

Your settings and logs stay in `%LOCALAPPDATA%\urakata`. Delete that
folder by hand if you do not need them.
