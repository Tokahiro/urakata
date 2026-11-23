# Changelog

The release workflow takes the notes of a release from the section with
its version. The heading line of each section is exactly `## <version>`.

## 0.1.3

- The About page of the settings window can now install an update: after
  **Look for updates**, press **Install**. The window closes during the
  update and opens again when it is done.

## 0.1.2

- New option `urakata --version` (short `-V`) prints the version.
- `urakata.exe`, `urakata-watch.exe` and `urakata-app.exe` now carry
  version information, shown in their file properties in Windows.

## 0.1.1

- Maintenance release with no change to the programs. It checks that
  the update from 0.1.0 works.

## 0.1.0

First public release.

urakata is a display helper for Steam Remote Play on Windows. Steam does
the streaming; urakata installs the SudoVDA driver, turns your other
displays off during a stream and brings your usual layout back after it.

- Watcher that starts at logon, with a tray icon
- Settings window with setup checks, watcher control and settings
- One-button driver repair
- Optional Steam library entry, usable from Big Picture and during a
  stream, with controller support
- Layout restore after a crash, a lost connection or sleep
- Update check against GitHub Releases
