//! Compiles the window (`ui/app.slint`) and puts the icons
//! (`assets/logo/icons.rc`) into the program, for Windows builds only.

fn main() {
    println!("cargo::rerun-if-changed=ui");
    println!("cargo::rerun-if-changed=../../assets/logo");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let config = slint_build::CompilerConfiguration::new()
        .with_edition((1, 19))
        .with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("ui/app.slint must compile");
    embed_resource::compile("../../assets/logo/icons.rc", embed_resource::NONE)
        .manifest_required()
        .expect("assets/logo/icons.rc must compile");
}
