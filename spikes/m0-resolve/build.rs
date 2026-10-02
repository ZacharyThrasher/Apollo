//! Locates the native libmpv import library (mpv.lib) and copies libmpv-2.dll next to the
//! produced binaries. Directory comes from APOLLO_LIBMPV_DIR (default: <repo>/tools/mpv-dev).
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=APOLLO_LIBMPV_DIR");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = env::var("APOLLO_LIBMPV_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("../../tools/mpv-dev"));
    let dir = dir.canonicalize().unwrap_or(dir);
    assert!(
        dir.join("mpv.lib").exists(),
        "mpv.lib not found in {} (see docs/build-baseline.md for how it is generated)",
        dir.display()
    );
    println!("cargo:rustc-link-search=native={}", dir.display());

    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out  ->  target/<profile>
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let profile_dir = out.ancestors().nth(3).unwrap().to_path_buf();
    let src = dir.join("libmpv-2.dll");
    let dst = profile_dir.join("libmpv-2.dll");
    let stale = match (fs::metadata(&src), fs::metadata(&dst)) {
        (Ok(s), Ok(d)) => s.len() != d.len(),
        _ => true,
    };
    if stale {
        fs::copy(&src, &dst).expect("copy libmpv-2.dll");
    }
}
