fn main() {
    println!("cargo:rerun-if-changed=assets/windows.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/windows.rc", embed_resource::NONE)
            .manifest_required()
            .expect("Windows icon resource compilation failed");
    }
}
