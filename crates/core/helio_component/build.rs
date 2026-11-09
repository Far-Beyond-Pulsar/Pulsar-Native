//! Link the C++ runtime for `intel_tex_2`'s prebuilt ASTC encoder (#1604).
//!
//! `intel_tex_2` (the BC7/ASTC texture compressor used by cooked textures)
//! ships `libispc_texcomp_astc<target>.a`, a precompiled C++ archive that
//! references libstdc++ (`__gxx_personality_v0`, `std::` symbols). Its own
//! build script, using prebuilt kernels, emits only `static=ispc_texcomp_astc...`
//! and never a C++ runtime, so a final link that has no other C++ dependency
//! (the generated game on Linux) fails with an undefined
//! `__gxx_personality_v0`. This crate is its sole user, so it supplies the
//! runtime; a native-lib directive from here lands after all rlibs, which is
//! the order the archive's symbols need.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => println!("cargo:rustc-link-lib=stdc++"),
        Ok("macos") | Ok("ios") => println!("cargo:rustc-link-lib=c++"),
        _ => {}
    }
}
