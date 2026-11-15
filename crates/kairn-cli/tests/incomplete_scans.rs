use std::fs;
use std::process::Command;

#[test]
fn incomplete_task_scans_fail_before_listing_or_mutating() {
    let vault = tempfile::tempdir().unwrap();
    let root = vault.path();
    fs::create_dir(root.join("Calendar")).unwrap();
    fs::create_dir(root.join("Notes")).unwrap();
    let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
    let source = root.join(format!("Calendar/{}.md", yesterday.format("%Y%m%d")));
    let original = "* move this\n* [x] completed\n";
    fs::write(&source, original).unwrap();
    fs::write(root.join("Notes/Broken.md"), [0xff]).unwrap();
    for args in [
        vec!["tasks"],
        vec!["tasks", "--done"],
        vec!["done", "move this"],
        vec!["carry"],
        vec!["carry", "--dry-run"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_kairn"))
            .arg("--root")
            .arg(root)
            .args(&args)
            .env("KAIRN_CONFIG", root.join("settings.json"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("incomplete vault scan"), "{error}");
        assert!(error.contains("Broken.md"), "{error}");
        assert_eq!(fs::read_to_string(&source).unwrap(), original);
        assert_eq!(fs::read_dir(root.join("Calendar")).unwrap().count(), 1);
        assert!(!root.join(".kairn").exists());
    }
}

#[test]
fn directory_errors_are_reported_in_json() {
    let vault = tempfile::tempdir().unwrap();
    fs::write(vault.path().join("Calendar"), "not a directory").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kairn"))
        .arg("--root")
        .arg(vault.path())
        .args(["--json", "tasks"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("incomplete vault scan"), "{output:?}");
    assert!(error.contains("Calendar"), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("\"count\":0"));
}
