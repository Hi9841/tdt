fn main() {
    println!("cargo:rerun-if-changed=tdt.rc");
    println!("cargo:rerun-if-changed=setup.manifest");
    println!("cargo:rerun-if-changed=../../desktop/assets/tdt.ico");
    // Without the asInvoker manifest, Windows sees "Setup" in the file name
    // and asks for admin rights, so a failed resource must stop the build.
    if let Err(error) = embed_resource::compile("tdt.rc", embed_resource::NONE).manifest_required()
    {
        panic!("{error}");
    }
}
