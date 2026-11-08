//! Runs the real `deskpane agent` setup commands. Every run passes
//! --settings with a temp file AND points HOME and USERPROFILE at a temp
//! folder, so the real ~/.claude/settings.json is never touched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

// the deskpane binary cargo built for this test run
const DESKPANE: &str = env!("CARGO_BIN_EXE_deskpane");

// a fresh, empty folder for one test
fn test_dir(name: &str) -> PathBuf {
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("dp-setup-{pid}-{name}"));
    let removed = fs::remove_dir_all(&dir);
    match removed {
        Ok(_) => {}
        Err(_) => {}
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

// runs deskpane with args, with the home folder moved into the test folder
fn run(args: &[&str], home: &Path) -> Output {
    Command::new(DESKPANE)
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .unwrap()
}

// the names of the files in a folder, sorted
fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        names.push(entry.unwrap().file_name().to_string_lossy().to_string());
    }
    names.sort();
    names
}

const THEIR_SETTINGS: &str = r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"/home/user/bin/notify.sh"}]}]}}"#;

#[test]
fn hooks_snippet_prints_every_event_in_exec_form() {
    let dir = test_dir("snippet");

    let output = run(
        &[
            "agent",
            "hooks-snippet",
            "--daemon",
            "192.168.1.50:8765",
            "--device-id",
            "dp-3f9a0c41",
        ],
        &dir,
    );

    assert_eq!(output.status.code(), Some(0));
    let snippet: Value = serde_json::from_slice(&output.stdout).unwrap();
    let hooks = snippet["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), 10);
    let handler = &snippet["hooks"]["PermissionRequest"][0]["hooks"][0];
    assert_eq!(handler["type"], "command");
    assert_eq!(handler["timeout"], 2);
    assert_eq!(handler["onFailure"], "continue");
    assert_eq!(
        handler["args"],
        serde_json::json!([
            "agent",
            "--daemon",
            "192.168.1.50:8765",
            "--device-id",
            "dp-3f9a0c41"
        ])
    );
    // the absolute path of this very binary
    let command = handler["command"].as_str().unwrap();
    assert!(Path::new(command).is_absolute());
    assert!(command.ends_with("deskpane") || command.ends_with("deskpane.exe"));
    assert_eq!(
        snippet["hooks"]["Notification"][0]["matcher"],
        "permission_prompt|idle_prompt|elicitation_dialog|elicitation_url_dialog|agent_needs_input"
    );
    assert!(files_in(&dir).is_empty(), "the snippet writes no file");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn hooks_snippet_refuses_a_daemon_without_port() {
    let dir = test_dir("snippet-noport");

    let output = run(
        &[
            "agent",
            "hooks-snippet",
            "--daemon",
            "192.168.1.50",
            "--device-id",
            "my-pc",
        ],
        &dir,
    );

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn install_then_uninstall_through_the_binary() {
    let dir = test_dir("install");
    let path = dir.join("settings.json");
    fs::write(&path, THEIR_SETTINGS).unwrap();
    let path_text = path.to_string_lossy().to_string();

    let installed = run(
        &[
            "agent",
            "install-hooks",
            "--daemon",
            "192.168.1.50:8765",
            "--device-id",
            "my-pc",
            "--settings",
            &path_text,
        ],
        &dir,
    );

    assert_eq!(installed.status.code(), Some(0));
    let stdout = String::from_utf8(installed.stdout).unwrap();
    assert!(stdout.contains("backup"));
    assert!(stdout.contains("Restart open Claude Code sessions"));
    let merged: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(merged["model"], "opus");
    assert_eq!(merged["hooks"]["Stop"].as_array().unwrap().len(), 2);
    // only the settings file and one backup; no .claude folder was made in "home"
    let names = files_in(&dir);
    assert_eq!(names.len(), 2);
    assert!(names[1].starts_with("settings.json.deskpane-backup-"));

    let uninstalled = run(
        &["agent", "uninstall-hooks", "--settings", &path_text],
        &dir,
    );

    assert_eq!(uninstalled.status.code(), Some(0));
    let back: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let original: Value = serde_json::from_str(THEIR_SETTINGS).unwrap();
    assert_eq!(back, original);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn install_refuses_invalid_json_and_exits_1() {
    let dir = test_dir("install-invalid");
    let path = dir.join("settings.json");
    fs::write(&path, "{not json").unwrap();
    let path_text = path.to_string_lossy().to_string();

    let output = run(
        &[
            "agent",
            "install-hooks",
            "--daemon",
            "192.168.1.50:8765",
            "--device-id",
            "my-pc",
            "--settings",
            &path_text,
        ],
        &dir,
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("hooks-snippet"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "{not json");
    assert_eq!(files_in(&dir), vec!["settings.json"]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn setup_command_with_a_bad_flag_is_a_usage_error() {
    // only the hook path swallows usage errors; setup commands report them
    let dir = test_dir("setup-badflag");

    let output = run(&["agent", "install-hooks", "--bogus"], &dir);

    assert_eq!(output.status.code(), Some(2));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn agent_test_sends_every_state_then_ended() {
    // the real `test` command takes about 10 s; run it against a local socket
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(4)))
        .unwrap();
    let addr = socket.local_addr().unwrap().to_string();
    let dir = test_dir("agent-test");

    let output = run(
        &["agent", "test", "--daemon", &addr, "--device-id", "my-pc"],
        &dir,
    );

    assert_eq!(output.status.code(), Some(0));
    let mut states: Vec<String> = Vec::new();
    let mut buf = [0u8; 1024];
    for _ in 0..6 {
        let (len, _) = socket.recv_from(&mut buf).unwrap();
        let packet: Value = serde_json::from_slice(&buf[..len]).unwrap();
        assert_eq!(packet["project"], "deskpane test");
        states.push(packet["state"].as_str().unwrap().to_string());
    }
    assert_eq!(
        states,
        vec!["idle", "working", "needs_you", "done", "error", "ended"]
    );
    fs::remove_dir_all(&dir).unwrap();
}
