//! The binary end to end, on the file store in a scratch home. `KEYFOB_TEST_KEYCHAIN=1` also runs the
//! macOS Keychain round trip (it stores, reads and deletes one item named keyfob-selftest-<pid>).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let e = Env { dir: tempfile::tempdir().unwrap() };
        std::fs::create_dir_all(e.home().join(".claude")).unwrap();
        e
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    fn decl(&self, body: &str) -> PathBuf {
        let p = self.dir.path().join("decl/keyfob.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_keyfob"));
        c.args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .env("HOME", self.home())
            .env("KEYFOB_BACKEND", "file")
            .env("KEYFOB_NO_PLUGINS", "1")
            .env("KEYFOB_DECLARATIONS", self.dir.path().join("decl"));
        c
    }
    fn run(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut c = self.cmd(args);
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.unwrap_or("").as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

const DECL: &str = r#"{
  "name": "demo-plugin",
  "secrets": { "demo-key": { "description": "a demo key", "obtain": "https://example.com/keys" } },
  "groups": {
    "demo": { "env": { "DEMO_KEY": "demo-key", "DEMO_SECRET": "demo-secret" }, "require_for": ["(^|\\s)demo-cli\\b"] }
  }
}"#;

#[test]
fn store_read_run_delete() {
    let e = Env::new();
    let o = e.run(&["add", "demo-key", "--stdin"], Some("value-123456789\n"));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!out(&o).contains("value-123456789"));

    // the file store is private
    let data = e.home().join(".local/share/keyfob");
    assert_eq!(mode(&data), 0o700);
    assert_eq!(mode(&data.join("secrets.json")), 0o600);
    assert_eq!(mode(&e.home().join(".config/keyfob/index.json")), 0o600);

    // ls never shows a value
    let ls = e.run(&["ls", "--json"], None);
    assert!(out(&ls).contains("demo-key") && !out(&ls).contains("value-123456789"));

    assert_eq!(out(&e.run(&["get", "demo-key"], None)), "value-123456789");

    // run hands it to one command, under its declared or default variable
    let ok = e.run(&["run", "demo-key", "--", "sh", "-c", "test \"$DEMO_KEY\" = value-123456789"], None);
    assert!(ok.status.success());
    let renamed = e.run(&["run", "demo-key=OTHER", "--", "sh", "-c", "test \"$OTHER\" = value-123456789"], None);
    assert!(renamed.status.success());

    // a missing secret stops the run before the command starts
    let miss = e.run(&["run", "nope", "--", "sh", "-c", "echo ran"], None);
    assert_eq!(miss.status.code(), Some(3));
    assert!(!out(&miss).contains("ran"));

    assert!(e.run(&["rename", "demo-key", "demo-key2"], None).status.success());
    assert_eq!(out(&e.run(&["get", "demo-key2"], None)), "value-123456789");
    assert!(e.run(&["rm", "demo-key2", "--yes"], None).status.success());
    assert_eq!(e.run(&["get", "demo-key2"], None).status.code(), Some(1));
}

#[test]
fn bad_input_is_refused() {
    let e = Env::new();
    assert!(!e.run(&["add", "Bad Name", "--stdin"], Some("x")).status.success());
    assert!(!e.run(&["add", "ok", "--stdin"], Some("   \n")).status.success());
    assert!(!e.run(&["add", "ok", "--stdin"], Some("two\nlines")).status.success());
    // no terminal and no --stdin: refuses instead of hanging
    assert!(!e.run(&["add", "ok"], None).status.success());
}

#[test]
fn groups_from_declarations() {
    let e = Env::new();
    e.decl(DECL);
    let c = e.run(&["check", "--json"], None);
    assert_eq!(c.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_str(&out(&c)).unwrap();
    assert_eq!(v["groups"]["demo"]["complete"], false);
    e.run(&["add", "demo-key", "--stdin"], Some("aaaaaaaaaa"));
    e.run(&["add", "demo-secret", "--stdin"], Some("bbbbbbbbbb"));
    let c = e.run(&["check", "--json"], None);
    assert!(c.status.success());
    let ok = e.run(&["run", "demo", "--", "sh", "-c", "test \"$DEMO_KEY$DEMO_SECRET\" = aaaaaaaaaabbbbbbbbbb && test \"$KEYFOB_ACTIVE\" = demo"], None);
    assert!(ok.status.success());
    assert!(out(&e.run(&["which", "DEMO_SECRET"], None)).contains("group demo, secret demo-secret"));
}

#[test]
fn capture_stores_and_scrubs() {
    let e = Env::new();
    e.decl(DECL);
    let msg = "here is my key\nkeyfob: demo-key k-1234567890abc DEMO_KEY\nand a github one ghp_abcdefghijklmnopqrstuvwxyz0123456789AB thanks";
    let o = e.run(&["capture"], Some(msg));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    let text = v["text"].as_str().unwrap();
    assert!(!text.contains("k-1234567890abc") && !text.contains("ghp_abcdef"));
    assert!(text.contains("keyfob: demo-key [stored in keyfob]") && text.contains("[keyfob:pasted-1]"));
    assert_eq!(v["stored"][0]["name"], "demo-key");
    assert_eq!(v["pending"][0]["name"], "pasted-1");
    assert_eq!(out(&e.run(&["get", "demo-key"], None)), "k-1234567890abc");
    // scrub replaces any stored value in a later output
    let s = e.run(&["scrub"], Some("log: token=k-1234567890abc ok"));
    assert_eq!(out(&s), "log: token=[keyfob:demo-key] ok");
}

#[test]
fn doctor_names_plaintext_without_values() {
    let e = Env::new();
    std::fs::create_dir_all(e.home()).unwrap();
    std::fs::write(e.home().join(".zshrc"), "export PATH=/x\nexport OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz\n").unwrap();
    std::fs::write(e.home().join(".claude/settings.json"), r#"{"env":{"MINERU_TOKEN":"eyJabcdefghijklmnop"}}"#).unwrap();
    let o = e.run(&["doctor", "--json"], None);
    let s = out(&o);
    assert!(s.contains("OPENAI_API_KEY") && s.contains("MINERU_TOKEN"));
    assert!(!s.contains("sk-abcdefghij") && !s.contains("eyJabcdefghij"));
}

#[test]
fn guard_hook_speaks_claude_codes_json() {
    let e = Env::new();
    e.decl(DECL);
    let ask = |cmd: &str| -> String { out(&e.run(&["guard"], Some(&serde_json::json!({ "tool_name": "Bash", "tool_input": { "command": cmd } }).to_string()))) };
    assert!(ask("keyfob get demo-key").contains("\"permissionDecision\":\"deny\""));
    assert!(ask("keyfob rm demo-key").contains("\"permissionDecision\":\"ask\""));
    assert_eq!(ask("ls -la"), "");
    // the group is not complete yet: demo-cli is not forced under keyfob run
    assert_eq!(ask("demo-cli ping"), "");
    e.run(&["add", "demo-key", "--stdin"], Some("aaaaaaaaaa"));
    e.run(&["add", "demo-secret", "--stdin"], Some("bbbbbbbbbb"));
    assert!(ask("demo-cli ping").contains("keyfob run demo --"));
    assert_eq!(ask("keyfob run demo -- demo-cli ping"), "");
    // anything else, or broken input, passes untouched
    assert_eq!(out(&e.run(&["guard"], Some(r#"{"tool_name":"Read","tool_input":{}}"#))), "");
    assert_eq!(out(&e.run(&["guard"], Some("not json"))), "");
}

#[test]
fn keychain_round_trip() {
    if !cfg!(target_os = "macos") || std::env::var("KEYFOB_TEST_KEYCHAIN").as_deref() != Ok("1") {
        return;
    }
    let e = Env::new();
    let name = format!("keyfob-selftest-{}", std::process::id());
    let kc = |args: &[&str], stdin: Option<&str>| {
        let mut c = e.cmd(args);
        // `security` finds the login Keychain through the real HOME; keyfob's own files stay in the scratch dir
        c.env("KEYFOB_BACKEND", "keychain")
            .env("HOME", std::env::var("HOME").unwrap())
            .env("KEYFOB_CONFIG_DIR", e.home().join(".config/keyfob"))
            .env("KEYFOB_DATA_DIR", e.home().join(".local/share/keyfob"));
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.unwrap_or("").as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    };
    let value = "p@ss \"quoted\" 'and' spaces ✓ $HOME";
    let a = kc(&["add", &name, "--stdin"], Some(value));
    assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));
    assert_eq!(out(&kc(&["get", &name], None)), value);
    assert!(kc(&["rm", &name, "--yes"], None).status.success());
    assert_eq!(kc(&["get", &name], None).status.code(), Some(1));
}
