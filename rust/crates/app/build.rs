fn main() {
    // Only when building for Windows; the icon and version details go into the exe's resources.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Swiftcord")
            .set("FileDescription", "Swiftcord")
            .set("LegalCopyright", "GPL-3.0-or-later");
        if let Err(e) = res.compile() {
            println!("cargo:warning=couldn't embed the icon: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
