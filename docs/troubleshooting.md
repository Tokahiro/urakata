# Troubleshooting

- [The stream shows my normal desktop](#the-stream-shows-my-normal-desktop-not-a-display-that-fits-my-client)
- [The picture is broken](#the-picture-is-broken-windows-are-missing-or-areas-are-green)
- [My other displays stay on during a stream](#my-other-displays-stay-on-during-a-stream)
- [My displays stay off after a stream](#my-displays-stay-off-after-a-stream)
- [Black band on my client](#there-is-a-black-band-above-or-beside-the-picture-on-my-client)
- [Error page after sleep](#steam-shows-an-error-page-after-the-pc-woke-from-sleep)
- [The driver needs a repair](#the-driver-needs-a-repair)
- [Steam debug port warning](#steam-debug-port-warning)
- [`steam-entry --add` says that Steam runs](#steam-entry---add-says-that-steam-runs)
- [SmartScreen](#windows-smartscreen-blocks-the-setup)
- [Logs and settings](#logs-and-settings)
- [Report a problem](#report-a-problem)

Start with the **Status** page of the settings window, or
`urakata status`. Most problems show up there as a yellow warning with a
fix.

## The stream shows my normal desktop, not a display that fits my client

Steam does not use the SudoVDA display.

- In Steam, open **Settings > Remote Play > Advanced Host Options** and
  set **Primary display while streaming** to **SudoVDA**. See
  [Setup](setup.md#1-set-steam-up).
- Check the **SudoVDA driver** line on the **Status** page. If the driver
  is missing or needs a repair, see
  [The driver needs a repair](#the-driver-needs-a-repair).

## The picture is broken: windows are missing or areas are green

Turn off **Use NVFBC capture on NVIDIA GPU** in Steam's **Advanced Host
Options**. NVFBC capture does not work well on the SudoVDA display. See
[Setup](setup.md#1-set-steam-up).

## My other displays stay on during a stream

- Check that the watcher runs: the **Watcher** page, or the tray icon.
  Start it there if it does not run.
- Check that **Turn other displays off during a stream** is on, on the
  **Settings** page.
- Steam sets its own display layout a few seconds after the stream
  starts. If your displays come back on at that moment, set a longer
  **Start delay**.
- If another program turns the displays on later, set a longer **Guard
  time** or more **Guard repeats**.

## My displays stay off after a stream

- If the client lost its connection (app closed, Wi-Fi gone), Steam ends
  the stream only after about 3 to 4 minutes. The displays come back
  then.
- To get them back at once, use one of these:
  - **Restore displays** in the tray menu.
  - **Restore** on the **Displays** page of the settings window.
  - `urakata restore` in a terminal.
- If urakata or the PC crashed during a stream, the watcher restores the
  layout at its next start, for example after you log on again.

## There is a black band above or beside the picture on my client

This is not urakata. The stream is often 16:9, and the Steam Link app
letterboxes it on a screen of another shape (for example the outer screen
of a foldable phone). If your client has **Display Style** in **Advanced
Client Options**, try **Zoom** or **Stretch** (not tested with urakata).

## Steam shows an error page after the PC woke from sleep

A stream ends when the PC goes to sleep. The first stream after wake can
show a Steam error page. Start the stream again; in our tests the second
try worked. urakata restores the displays after wake.

## The driver needs a repair

The **SudoVDA driver** check says "needs repair", or Steam cannot make
the virtual display.

1. End any stream.
2. Open the settings window, go to **Displays** and press **Repair** (or
   **Install** if the driver is missing). Windows asks for admin rights
   once.
3. The result shows in the window. Details are in `driver-repair.log` in
   the log folder (the **Open** button on the **Displays** page).

Or, from a terminal that runs as administrator:

```powershell
& "$env:LOCALAPPDATA\Programs\urakata\urakata.exe" driver repair
```

If you elevate with another user's account, type the full path of your
own `urakata.exe` instead.

If the repair does not help, run the setup again and choose **Reinstall
the bundled driver** on the driver page.

## Steam debug port warning

The **Steam debug port** check warns when the file
`.cef-enable-remote-debugging` is in the Steam folder. Then Steam opens a
remote debugging port at its next start, and other programs (maybe also
web pages) can control the Steam client. urakata does not need this
port. Some other tools add the file. If you do not use such a tool,
delete the file and restart Steam.

## `steam-entry --add` says that Steam runs

Steam must be closed to change its library. Exit Steam with
**Steam > Exit** (closing the window is not enough), then run the command
again.

## Windows SmartScreen blocks the setup

See [Install: SmartScreen](install.md#smartscreen).

## Logs and settings

All files are in `%LOCALAPPDATA%\urakata` (the **Open** button on the
**Displays** page):

| File | Content |
| --- | --- |
| `watch.log` | What the watcher did, with times. |
| `settings.json` | The watcher settings. |
| `session.json` | The saved display layout during a stream. |
| `driver-repair.log` | The result of the last driver repair from the settings window. |

The setup writes its driver log to `driver.log` in
`%LOCALAPPDATA%\Programs\urakata`.

## Report a problem

Open an [issue](https://github.com/Tokahiro/urakata/issues). Add:

- your Windows version, graphics card and client device,
- the output of `urakata status`,
- the part of `watch.log` around the problem.

Logs can contain the names of your displays, your client devices and
your Windows user folder. Remove what you do not want to share before you
post.
