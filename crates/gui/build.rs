//! Compiles the `.slint` files with the built-in style that looks most at
//! home on the target system. The style is fixed at build time, so this reads
//! the target from Cargo instead of `cfg`, which would describe the build
//! machine when cross compiling.

fn main() {
    let style = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => "fluent",
        Ok("macos") => "cupertino",
        // Fluent is the most neutral of the rest next to GNOME and KDE, and
        // the only one besides Qt that ships a polished table and menus.
        _ => "fluent",
    };
    // SLINT_STYLE still wins, to preview another style without editing this.
    let style = std::env::var("SLINT_STYLE").unwrap_or_else(|_| style.to_string());
    println!("cargo:rerun-if-env-changed=SLINT_STYLE");
    let config = slint_build::CompilerConfiguration::new().with_style(style);
    slint_build::compile_with_config("ui/app.slint", config).expect("compiling the UI");
}
