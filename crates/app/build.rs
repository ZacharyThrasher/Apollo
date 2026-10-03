//! Embeds the app icon (and nothing else) into the Windows executable. A machine without a
//! resource compiler still builds; the exe just keeps the default icon.

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/apollo.ico");
    if let Err(err) = embed_resource::compile("assets/app.rc", embed_resource::NONE).manifest_optional() {
        println!("cargo:warning=app icon not embedded: {err}");
    }
}
