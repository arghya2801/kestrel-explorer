fn main() {
    println!("cargo:rerun-if-changed=res/app.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:res/app.manifest");
}
