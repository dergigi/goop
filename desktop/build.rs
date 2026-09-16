fn main() {
    println!("cargo:rerun-if-changed=resources/goop.rc");
    println!("cargo:rerun-if-changed=resources/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // GPUI loads icon resource 1. Explorer and installer shortcuts also use
        // the executable's first icon, so embed it in the executable itself.
        embed_resource::compile_for("resources/goop.rc", ["goop"], embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed the Windows application icon");
    }
}
