#[cfg(feature = "unix-mtp")]
fn build_unix_mtp() {
    if !std::env::var("CARGO_CFG_TARGET_FAMILY")
        .is_ok_and(|families| families.split(',').any(|family| family == "unix"))
    {
        return;
    }
    const SHIM: &str = "mtp_unix.c";
    println!("cargo:rerun-if-changed={SHIM}");
    let mut config = pkg_config::Config::new();
    config.atleast_version("1.1.21").cargo_metadata(false);
    let libmtp = config.probe("libmtp").expect(
        "unix-mtp requires stock libmtp >= 1.1.21 and its development headers via pkg-config",
    );
    let mut build = cc::Build::new();
    build.file(SHIM).includes(libmtp.include_paths);
    for (name, value) in &libmtp.defines {
        build.define(name, value.as_deref());
    }
    build.compile("sliploom_unix_mtp");
    config
        .cargo_metadata(true)
        .probe("libmtp")
        .expect("libmtp link flags");
}

fn main() {
    #[cfg(feature = "unix-mtp")]
    build_unix_mtp();

    const ICON: &str = "sliploom-icon.res";
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
        let resource = std::path::Path::new(&manifest).join(ICON);
        let resource = resource.display();
        println!("cargo:rustc-link-arg-bin=sliploom={resource}");
    }
}
