//! Windows resources of the programs: the icons (`assets/logo/icons.rc`)
//! and a version resource with the Cargo version, which Explorer shows
//! under Properties > Details and Task Manager shows as the name.
//! Shared by the build scripts of `urakata` and `urakata-app`.

use std::path::Path;

/// Puts the icons and the version resource into the program `bin`.
/// `description` is the file description, the name Task Manager shows.
pub fn embed(bin: &str, description: &str) {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let logo_dir = Path::new(&manifest_dir).join("../../assets/logo");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    // embed-resource names its output after the .rc file, so each program
    // needs its own file.
    let rc = Path::new(&out_dir).join(format!("{bin}.rc"));
    std::fs::write(&rc, resource_script(bin, description)).expect("write the .rc file");
    embed_resource::compile_for(
        &rc,
        [bin],
        embed_resource::ParamsIncludeDirs([logo_dir.as_os_str()]),
    )
    .manifest_required()
    .expect("the resources must compile");
}

fn resource_script(bin: &str, description: &str) -> String {
    let var = |name: &str| std::env::var(name).expect(name);
    // Cargo gives the numbers without the pre-release part ("-beta.1").
    let numbers = format!(
        "{},{},{},0",
        var("CARGO_PKG_VERSION_MAJOR"),
        var("CARGO_PKG_VERSION_MINOR"),
        var("CARGO_PKG_VERSION_PATCH"),
    );
    let version = var("CARGO_PKG_VERSION");
    // VS_FF_PRERELEASE for versions such as "0.2.0-beta.1".
    let flags = if var("CARGO_PKG_VERSION_PRE").is_empty() {
        "0x0"
    } else {
        "0x2"
    };
    // Numbers instead of the winver.h names, so no SDK header is needed:
    // FILEOS 0x40004 = VOS_NT_WINDOWS32, FILETYPE 1 = VFT_APP,
    // translation 0x409/1200 = English (US), Unicode.
    format!(
        r#"#include "icons.rc"

1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEFLAGSMASK 0x3F
FILEFLAGS {flags}
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "FileDescription", "{description}"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "{bin}"
            VALUE "OriginalFilename", "{bin}.exe"
            VALUE "ProductName", "urakata"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    )
}
