fn main() {
    const ICON: &str = "seam-icon.res";
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
        let resource = std::path::Path::new(&manifest).join(ICON);
        let resource = resource.display();
        println!("cargo:rustc-link-arg-bin=seam-desktop={resource}");
    }
}
