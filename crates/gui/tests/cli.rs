//! The flags installers run. They must work with no display at all, since
//! an installer may run them before anyone logs in.

use std::process::Command;

fn gui() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hatchzip-gui"));
    // No display to open a window on, so any attempt would fail the run.
    cmd.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
    cmd
}

#[test]
fn register_and_unregister_without_a_window() {
    // Elsewhere the entries go to the real registry; the Windows installer
    // test covers that.
    if std::env::consts::OS != "linux" {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let menu = data.path().join("kio/servicemenus/hatchzip.desktop");

    let out = gui()
        .arg("--register-shell")
        .env("XDG_DATA_HOME", data.path())
        .env_remove("APPIMAGE")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let printed = String::from_utf8(out.stdout).unwrap();
    assert!(printed.contains("hatchzip.desktop"), "{printed}");
    let entry = std::fs::read_to_string(&menu).unwrap();
    assert!(
        entry.contains(env!("CARGO_BIN_EXE_hatchzip-gui")),
        "{entry}"
    );

    let out = gui()
        .arg("--unregister-shell")
        .env("XDG_DATA_HOME", data.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(!menu.exists());
}

#[test]
fn an_appimage_registers_itself_not_its_mount() {
    if std::env::consts::OS != "linux" {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let out = gui()
        .arg("--register-shell")
        .env("XDG_DATA_HOME", data.path())
        .env("APPIMAGE", "/home/sam/Apps/HatchZip-x86_64.AppImage")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let entry =
        std::fs::read_to_string(data.path().join("kio/servicemenus/hatchzip.desktop")).unwrap();
    assert!(
        entry.contains("/home/sam/Apps/HatchZip-x86_64.AppImage"),
        "{entry}"
    );
    assert!(!entry.contains(env!("CARGO_BIN_EXE_hatchzip-gui")));
}
