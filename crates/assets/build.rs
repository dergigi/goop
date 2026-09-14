fn main() {
    // Track the directory as well as existing embedded files so adding or removing
    // an asset invalidates RustEmbed's generated file list in incremental builds.
    println!("cargo:rerun-if-changed=../../assets");
}
