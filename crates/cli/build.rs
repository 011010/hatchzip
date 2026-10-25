//! On Windows, embeds a version resource in hatchzip.exe: the product name,
//! company, description and version that Explorer shows under Properties >
//! Details, and that code signing services and antivirus engines read. It
//! depends on the target, not the build machine, so this reads the target
//! from Cargo instead of `cfg`.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The same names as hatchzip-gui's; winresource fills in the
        // version from the package.
        winresource::WindowsResource::new()
            .set("ProductName", "HatchZip")
            .set("FileDescription", "HatchZip command line")
            .set("CompanyName", "HatchZip")
            .set("LegalCopyright", "MIT OR Apache-2.0")
            .compile()
            .expect("embedding the Windows version resource");
    }
}
