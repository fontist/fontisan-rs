//! #158 break test F4: one unreadable subdirectory lost the whole batch.

/// `Dir.glob("**/*")` skips a directory it cannot read; so must the walk.
#[cfg(unix)]
#[test]
fn an_unreadable_subdirectory_does_not_lose_the_batch() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("fontisan-p1break-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ok")).unwrap();
    std::fs::create_dir_all(dir.join("locked")).unwrap();
    std::fs::write(dir.join("a.ttf"), b"x").unwrap();
    std::fs::write(dir.join("ok/b.otf"), b"x").unwrap();
    std::fs::write(dir.join("locked/c.ttf"), b"x").unwrap();
    std::fs::set_permissions(dir.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let locked = std::fs::read_dir(dir.join("locked")).is_err(); // false when running as root
    let got = fontisan::scan::font_files(&dir);
    let rows = fontisan::scan::scan_dir(&dir);
    std::fs::set_permissions(dir.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let names: Vec<_> = got
        .unwrap()
        .iter()
        .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned())
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(rows.is_ok(), "scan_dir must not fail the whole batch");
    let want: &[&str] = if locked {
        &["a.ttf", "ok/b.otf"]
    } else {
        &["a.ttf", "locked/c.ttf", "ok/b.otf"]
    };
    assert_eq!(names, want);
}
