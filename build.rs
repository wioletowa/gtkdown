// Bundles the icons into the binary (glib-compile-resources ships with GLib).
use std::process::Command;

fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    println!("cargo:rerun-if-changed=data");
    let status = Command::new("glib-compile-resources")
        .args(["--sourcedir=data/icons", &format!("--target={out}/icons.gresource"), "data/icons.gresource.xml"])
        .status()
        .expect("glib-compile-resources not found (it comes with GLib)");
    assert!(status.success(), "glib-compile-resources failed");
}
