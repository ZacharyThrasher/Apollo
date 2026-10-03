//! Windows only: finds the native import library (mpv.lib) and puts libmpv-2.dll next to every
//! binary cargo produces (profile dir for bins, deps/ for test executables).
//! Directory: APOLLO_LIBMPV_DIR, default <repo>/tools/mpv-dev (see scripts/setup-m0-tools.ps1).
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=APOLLO_LIBMPV_DIR");
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = env::var("APOLLO_LIBMPV_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("../../tools/mpv-dev"));
    let dir = dir.canonicalize().unwrap_or(dir);
    assert!(
        dir.join("mpv.lib").exists(),
        "mpv.lib not found in {} (run scripts/setup-m0-tools.ps1)",
        dir.display()
    );
    println!("cargo:rustc-link-search=native={}", dir.display());

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let profile_dir = out.ancestors().nth(3).unwrap().to_path_buf();
    let src = dir.join("libmpv-2.dll");
    for dst_dir in [profile_dir.clone(), profile_dir.join("deps")] {
        let dst = dst_dir.join("libmpv-2.dll");
        let stale = match (fs::metadata(&src), fs::metadata(&dst)) {
            (Ok(s), Ok(d)) => s.len() != d.len(),
            _ => true,
        };
        if stale && dst_dir.exists() {
            fs::copy(&src, &dst).expect("copy libmpv-2.dll");
        }
    }
}
