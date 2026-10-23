//! Registration writes exactly what each file manager reads, pointing at the
//! right program, and unregistering leaves nothing behind.

use std::path::Path;

use hatchzip_shell::{Dirs, is_registered, register};

#[cfg(target_os = "linux")]
#[test]
fn linux_menus_point_at_the_gui_and_go_away_cleanly() {
    use std::os::unix::fs::PermissionsExt;

    use hatchzip_shell::{ACTIONS, unregister};

    let tmp = tempfile::TempDir::new().unwrap();
    let dirs = Dirs {
        data: tmp.path().to_path_buf(),
    };
    // A path with a space and a quote, the classic way to break Exec lines.
    let gui = Path::new("/opt/Hatch Zip/it's/hatchzip-gui");

    assert!(!is_registered(&dirs));
    register(&dirs, gui).unwrap();
    assert!(is_registered(&dirs));

    let kde = tmp.path().join("kio/servicemenus/hatchzip.desktop");
    let text = std::fs::read_to_string(&kde).unwrap();
    for a in ACTIONS {
        assert!(
            text.contains(&format!("Exec=\"{}\" {} %f", gui.display(), a.flag)),
            "{}",
            a.id
        );
    }
    assert_eq!(
        std::fs::metadata(&kde).unwrap().permissions().mode() & 0o111,
        0o111
    );

    let script = tmp.path().join("nautilus/scripts/HatchZip - Extract here");
    let text = std::fs::read_to_string(&script).unwrap();
    assert!(text.contains("'/opt/Hatch Zip/it'\\''s/hatchzip-gui' --extract-here"));
    assert!(std::fs::metadata(&script).unwrap().permissions().mode() & 0o100 != 0);

    let nemo = tmp.path().join("nemo/actions/hatchzip-test.nemo_action");
    assert!(
        std::fs::read_to_string(&nemo)
            .unwrap()
            .contains("Extensions=zip;7z;tar;tgz;gz;001;")
    );

    // Running it twice is harmless, and so is unregistering twice.
    register(&dirs, gui).unwrap();
    unregister(&dirs).unwrap();
    unregister(&dirs).unwrap();
    assert!(!is_registered(&dirs));
    let left: Vec<_> = walk(tmp.path());
    assert!(left.is_empty(), "left behind: {left:?}");
}

#[cfg(target_os = "linux")]
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            files.extend(walk(&p));
        } else {
            files.push(p);
        }
    }
    files
}

#[cfg(windows)]
#[test]
fn explorer_menu_points_at_the_gui_and_goes_away_cleanly() {
    use hatchzip_shell::{ACTIONS, unregister};
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let dirs = Dirs::current().unwrap();
    let gui = Path::new(r"C:\Program Files\Hatch Zip\hatchzip-gui.exe");
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let base = r"Software\Classes\SystemFileAssociations";

    register(&dirs, gui).unwrap();
    assert!(is_registered(&dirs));
    for ext in hatchzip_shell::EXTENSIONS {
        let menu = hkcu
            .open_subkey(format!(r"{base}\.{ext}\shell\HatchZip"))
            .unwrap();
        assert_eq!(menu.get_value::<String, _>("MUIVerb").unwrap(), "HatchZip");
        for (i, a) in ACTIONS.iter().enumerate() {
            let cmd = menu
                .open_subkey(format!(r"shell\{:02}{}\command", i + 1, a.id))
                .unwrap();
            let line: String = cmd.get_value("").unwrap();
            assert_eq!(line, format!("\"{}\" {} \"%1\"", gui.display(), a.flag));
        }
    }

    unregister(&dirs).unwrap();
    unregister(&dirs).unwrap();
    assert!(!is_registered(&dirs));
    for ext in hatchzip_shell::EXTENSIONS {
        assert!(
            hkcu.open_subkey(format!(r"{base}\.{ext}\shell\HatchZip"))
                .is_err()
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_says_it_is_not_supported_yet() {
    let dirs = Dirs::current().unwrap();
    assert!(register(&dirs, Path::new("/Applications/HatchZip.app")).is_err());
    assert!(!is_registered(&dirs));
}
