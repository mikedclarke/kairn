use std::fs;
use std::process::Command;

#[test]
fn cli_resumes_the_persisted_transfer_before_scanning_for_new_work() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::create_dir(root.join("Calendar")).unwrap();
    fs::create_dir_all(root.join(".kairn/local")).unwrap();
    let today = chrono::Local::now().date_naive();
    let yesterday = today - chrono::Duration::days(1);
    let destination = format!("Calendar/{}.md", today.format("%Y%m%d"));
    let source = format!("Calendar/{}.md", yesterday.format("%Y%m%d"));
    let before = "* duplicate\n";
    let after = "* duplicate\n* duplicate\n";
    fs::write(root.join(&destination), after).unwrap();
    fs::write(root.join(&source), before).unwrap();
    let plan = serde_json::json!({
        "version": 1,
        "root": root,
        "id": "carry-test-operation",
        "section": null,
        "report": {"to": today.to_string(), "file": destination, "tasks": [{"text": "duplicate"}], "older_than_window": 0},
        "steps": [
            {"path": destination, "before": before, "after": after, "complete": false},
            {"path": source, "before": before, "after": "", "complete": false},
        ],
        "activity": [{"ts": "2026-08-07 12:00:00", "actor": "test", "action": "carry", "target": destination, "detail": "duplicate"}],
    });
    fs::write(root.join(".kairn/local/carry.json"), plan.to_string()).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_kairn"))
            .arg("--root")
            .arg(&root)
            .args(["--json", "carry"])
            .env("KAIRN_CONFIG", root.join("settings.json"))
            .output()
            .unwrap()
    };
    let output = run();
    assert!(output.status.success(), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["resumed"], true);
    assert_eq!(report["count"], 1);
    assert_eq!(fs::read_to_string(root.join(&destination)).unwrap(), after);
    assert_eq!(fs::read_to_string(root.join(&source)).unwrap(), "");
    assert_eq!(kairn_core::recent_activity(&root, 10).len(), 1);
    assert!(!root.join(".kairn/local/carry.json").exists());
    let second = run();
    assert!(second.status.success(), "{second:?}");
    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report["count"], 0);
    assert_eq!(fs::read_to_string(root.join(&destination)).unwrap(), after);
}

#[test]
fn new_carry_preserves_sections_and_newline_style() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("Calendar")).unwrap();
    let today = chrono::Local::now().date_naive();
    let yesterday = today - chrono::Duration::days(1);
    let destination = kairn_core::daily_path(root, today);
    let source = kairn_core::daily_path(root, yesterday);
    fs::write(
        &destination,
        "## Tasks\r\n* already here\r\n\r\n## Log\r\nunchanged\r\n",
    )
    .unwrap();
    fs::write(
        &source,
        "**Group**\r\n* move this\r\n\r\n## Log\r\nkeep this\r\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kairn"))
        .arg("--root")
        .arg(root)
        .args(["carry", "--section", "Tasks"])
        .env("KAIRN_CONFIG", root.join("settings.json"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(destination).unwrap(),
        "## Tasks\r\n* already here\r\n**Group**\r\n* move this\r\n\r\n## Log\r\nunchanged\r\n"
    );
    assert_eq!(
        fs::read_to_string(source).unwrap(),
        "\r\n## Log\r\nkeep this\r\n"
    );
}
