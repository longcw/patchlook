fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon/patchlook.rc");
    println!("cargo:rerun-if-changed=assets/app-icon/patchlook.ico");
    embed_resource::compile_for(
        "assets/app-icon/patchlook.rc",
        [env!("CARGO_PKG_NAME")],
        embed_resource::NONE,
    )
    .manifest_required()
    .expect("Cannot embed the Windows application icon");
}
