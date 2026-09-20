fn main() {
    println!("cargo:rerun-if-changed=assets/tdt.ico");
    println!("cargo:rerun-if-changed=assets/tdt.rc");
    embed_resource::compile("assets/tdt.rc", embed_resource::NONE)
        .manifest_optional()
        .ok();
}
