# Contributing

Bug reports and ideas are welcome as
[issues](https://github.com/Tokahiro/urakata/issues). For a bug, add your
Windows version, graphics card, client device and the output of
`urakata status` (see [Report a problem](docs/troubleshooting.md#report-a-problem)).

## Build

urakata builds on Windows only (x64). The settings window
(`urakata-app`) needs Rust 1.95 or newer, the rest 1.85.

```powershell
cargo build --release
```

Before you open a pull request, run the same checks as CI:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Most of urakata talks to Windows, the SudoVDA driver and Steam, so unit
tests cover only the logic around them. Please test a change that touches
displays, the driver or Steam on a real host with a stream, and say in
the pull request what you tested.

## Layout

| Path | Content |
| --- | --- |
| `crates/sudovda` | The SudoVDA control interface. The `host-test` feature and `examples/fake-display.rs` add a virtual display without Steam, for tests on a real host. |
| `crates/urakata` | `urakata.exe` (command line) and `urakata-watch.exe` (watcher and tray icon). |
| `crates/urakata-app` | `urakata-app.exe`, the settings window ([Slint](https://slint.dev)). |
| `installer` | Inno Setup script, the driver download script and third-party notices. |
| `assets` | Logo, Steam library artwork and the scripts that make them. |

## Installer and releases

The installer is built with Inno Setup 6.3 or newer from
`installer/urakata.iss`. `installer/fetch-driver.ps1` downloads Apollo
0.4.6, takes the SudoVDA driver files from it and checks their hashes.
The repository holds no driver binary.

`.github/workflows/release.yml` builds the programs, the driver package
and the installer:

- on a pull request that changes `installer/` or the workflow, as a
  workflow artifact;
- on a tag `v<version>`, as a draft GitHub release with
  `urakata-setup-<version>.exe` and its `.sha256` file. The tag must match
  the version in `Cargo.toml`. The release notes are the `## <version>`
  section of `CHANGELOG.md`; the build fails without it.

To release: raise `version` in `Cargo.toml`, add its section to
`CHANGELOG.md`, merge, tag `v<version>` on `main`, push the tag, check
the draft release and publish it.

## License

By contributing, you agree that your work is released under the
[GPL-3.0](LICENSE).
