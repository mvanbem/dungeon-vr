use std::env;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let steam_audio_path = env::var("STEAM_AUDIO_PATH").unwrap();
    let steam_audio_lib_dir = env::var("STEAM_AUDIO_LIB_DIR").unwrap();

    println!("cargo:rustc-link-lib=dylib=phonon");
    println!("cargo:rustc-link-search=native={steam_audio_lib_dir}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=STEAM_AUDIO_PATH");
    println!("cargo:rerun-if-env-changed=STEAM_AUDIO_LIB_DIR");

    bindgen::builder()
        .clang_args([format!("-I{steam_audio_path}")])
        .header(format!("{steam_audio_path}/phonon.h"))
        .header(format!("{steam_audio_path}/phonon_version.h"))
        .bitfield_enum("(.*)Flags")
        .generate()
        .unwrap()
        .write_to_file(format!("{out_dir}/bindgen.rs"))
        .unwrap();
}
