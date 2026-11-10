//! Embute `island.exe.manifest` no executável (só Windows). Ver o comentário no manifest.

fn main() {
    println!("cargo:rerun-if-changed=island.exe.manifest");
    println!("cargo:rerun-if-changed=island.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("island.rc", embed_resource::NONE).manifest_required().unwrap();
    }
}
