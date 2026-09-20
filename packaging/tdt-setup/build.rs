fn main() {
    println!("cargo:rerun-if-changed=tdt.rc");
    println!("cargo:rerun-if-changed=../../desktop/assets/tdt.ico");
    embed_resource::compile("tdt.rc", embed_resource::NONE)
        .manifest_optional()
        .ok();
}
