//! Puts the icons and the version resource into the programs, for
//! Windows builds only (see `winres.rs`).

mod winres;

fn main() {
    println!("cargo::rerun-if-changed=../../assets/logo");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    winres::embed("urakata", "urakata command line");
    winres::embed("urakata-watch", "urakata watcher");
}
