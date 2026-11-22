fn main() {
    println!("cargo:rerun-if-changed=kindle-stripper.rc");
    println!("cargo:rerun-if-changed=kindle-stripper.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for(
            "kindle-stripper.rc",
            ["kindle-stripper"],
            embed_resource::NONE,
        )
        .manifest_required()
        .expect("embed the Kindle Stripper icon");
    }
}
