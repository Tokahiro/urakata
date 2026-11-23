//! Puts the icons (`assets/logo/icons.rc`) into the programs, for Windows
//! builds only.

fn main() {
    println!("cargo::rerun-if-changed=../../assets/logo");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    embed_resource::compile("../../assets/logo/icons.rc", embed_resource::NONE)
        .manifest_required()
        .expect("assets/logo/icons.rc must compile");
}
