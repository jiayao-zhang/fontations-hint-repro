use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if !cfg!(feature = "bundled") {
        let target = env::var("TARGET").unwrap();
        if !target.contains("android") && !target.contains("ohos") {
            pkg_config::Config::new()
                .atleast_version("24.3.18")
                .probe("freetype2")
                .unwrap();
        }
        return;
    }

    // Build the vendored FreeType with CMake directly (no libpng/zlib/brotli/harfbuzz), keeping
    // the reproduction free of extra system libraries.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let build_dir = out_dir.join("freetype-build");
    let temp_dir = out_dir.join("msbuild-temp");
    std::fs::create_dir_all(&temp_dir).expect("create build temp dir");
    let source_dir = Path::new("freetype2");
    println!("cargo:rerun-if-changed=build.rs");

    // MSBuild writes temporary batch files and may not be able to use the user temp folder in
    // restricted environments, so point it at a directory inside OUT_DIR.
    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(source_dir)
        .arg("-B")
        .arg(&build_dir)
        .arg("-A")
        .arg("x64")
        .arg("-DBUILD_SHARED_LIBS=OFF")
        .arg("-DFT_DISABLE_PNG=ON")
        .arg("-DFT_DISABLE_ZLIB=ON")
        .arg("-DFT_DISABLE_BZIP2=ON")
        .arg("-DFT_DISABLE_BROTLI=ON")
        .arg("-DFT_DISABLE_HARFBUZZ=ON")
        .env("TEMP", &temp_dir)
        .env("TMP", &temp_dir);
    let status = configure.status().expect("failed to run cmake configure");
    assert!(status.success(), "cmake configure failed");

    let mut build = Command::new("cmake");
    build
        .arg("--build")
        .arg(&build_dir)
        .arg("--config")
        .arg("Release")
        .env("TEMP", &temp_dir)
        .env("TMP", &temp_dir);
    let status = build.status().expect("failed to run cmake build");
    assert!(status.success(), "cmake build failed");

    for candidate in [
        build_dir.join("Release"),
        build_dir.join("lib"),
        build_dir.clone(),
    ] {
        if candidate.join("freetype.lib").exists() || candidate.join("libfreetype.a").exists() {
            println!("cargo:rustc-link-search=native={}", candidate.display());
        }
    }
    println!("cargo:rustc-link-lib=static=freetype");
}
