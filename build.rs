fn main() {
    println!("cargo:rerun-if-changed=res/app.manifest");
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/app.ico");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:res/app.manifest");
    // Window tests get the same common controls version as the app (list groups need v6).
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:res/app.manifest");
    // Icon and version info. Fails the build loudly if the resource compiler is missing.
    embed_resource::compile("res/app.rc", embed_resource::NONE).manifest_optional().unwrap();
}
