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
fn doctor_follows_sourced_files_and_reads_mcp_config() {
    let e = Env::new();
    std::fs::create_dir_all(e.home().join(".config")).unwrap();
    std::fs::write(e.home().join(".bashrc"), "[ -f \"$HOME/.config/api-keys.env\" ] && . \"$HOME/.config/api-keys.env\"\n").unwrap();
    std::fs::write(e.home().join(".config/api-keys.env"), "export S2_API_KEY=abcdefghijklmnop\nexport OPENALEX_API_KEY=qrstuvwxyz123456\n").unwrap();
    std::fs::write(e.home().join(".claude.json"), r#"{"mcpServers":{"mineru":{"type":"http","url":"https://x","env":{"MINERU_API_TOKEN":"eyJabcdefghijklmnopqrstu"}}}}"#).unwrap();
    let s = out(&e.run(&["doctor", "--json"], None));
    assert!(s.contains("S2_API_KEY") && s.contains("OPENALEX_API_KEY") && s.contains("api-keys.env"), "{s}");
    assert!(s.contains("mcpServers.mineru.env") && s.contains("MINERU_API_TOKEN"), "{s}");
    assert!(!s.contains("abcdefghij") && !s.contains("eyJabcdef"));
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

#[test]
fn declarations_of_a_folder_marketplace_come_from_its_source() {
    // installed_plugins.json points at a stale cached copy without keyfob.json; the marketplace is a local
    // folder that Claude Code loads in place, and the declaration lives in that source.
    let e = Env::new();
    let root = e.dir.path();
    let claude = root.join("claude");
    let market = root.join("market");
    let stale = root.join("cache/desk/0.1.0");
    std::fs::create_dir_all(market.join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(market.join("plugins/desk")).unwrap();
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::create_dir_all(claude.join("plugins")).unwrap();
    std::fs::write(market.join(".claude-plugin/marketplace.json"), r#"{"name":"m","plugins":[{"name":"desk","source":"./plugins/desk"}]}"#).unwrap();
    std::fs::write(market.join("plugins/desk/keyfob.json"), r#"{"name":"desk","groups":{"g":{"env":{"G_KEY":"g-key"}}}}"#).unwrap();
    std::fs::write(claude.join("plugins/installed_plugins.json"), serde_json::json!({"version":2,"plugins":{"desk@m":[{"installPath": stale}]}}).to_string()).unwrap();
    std::fs::write(claude.join("plugins/known_marketplaces.json"), serde_json::json!({"m":{"source":{"source":"directory","path": market}}}).to_string()).unwrap();
    std::fs::write(claude.join("settings.json"), r#"{"enabledPlugins":{"desk@m":true}}"#).unwrap();
    let mut c = e.cmd(&["decls", "--json"]);
    c.env_remove("KEYFOB_NO_PLUGINS").env("CLAUDE_CONFIG_DIR", &claude);
    let o = c.output().unwrap();
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["groups"]["g"]["env"]["G_KEY"], "g-key", "{}", out(&o));
    // disabled: not read
    std::fs::write(claude.join("settings.json"), r#"{"enabledPlugins":{"desk@m":false}}"#).unwrap();
    let mut c = e.cmd(&["decls", "--json"]);
    c.env_remove("KEYFOB_NO_PLUGINS").env("CLAUDE_CONFIG_DIR", &claude);
    let v: serde_json::Value = serde_json::from_str(&out(&c.output().unwrap())).unwrap();
    assert!(v["groups"]["g"].is_null());
}

const OPTIONAL_DECL: &str = r#"{
  "name": "litrev",
  "groups": {
    "lit": { "env": { "OPENALEX_API_KEY": "openalex-key", "S2_API_KEY": "s2-key", "MUST": "must-key" },
             "optional": ["OPENALEX_API_KEY", "S2_API_KEY"] },
    "odd": { "env": { "A": "a-key" }, "optional": ["NOT_A_VAR"] }
  }
}"#;

fn check_json(e: &Env) -> serde_json::Value {
    serde_json::from_str(&out(&e.run(&["check", "--json"], None))).unwrap()
}

#[test]
fn optional_group_members() {
    let e = Env::new();
    e.decl(OPTIONAL_DECL);
    // only a required member decides completeness and whether run starts
    let v = check_json(&e);
    assert_eq!(v["groups"]["lit"]["complete"], false);
    assert_eq!(v["groups"]["lit"]["missing"], serde_json::json!(["must-key"]));
    assert_eq!(v["groups"]["lit"]["optional_missing"], serde_json::json!(["openalex-key", "s2-key"]));
    let miss = e.run(&["run", "lit", "--", "sh", "-c", "echo ran"], None);
    assert_eq!(miss.status.code(), Some(3));
    let err = String::from_utf8_lossy(&miss.stderr).into_owned();
    assert!(err.contains("must-key") && !err.contains("s2-key"), "{err}");
    e.run(&["add", "must-key", "--stdin"], Some("mmmmmmmm"));
    e.run(&["add", "openalex-key", "--stdin"], Some("oooooooo"));
    let v = check_json(&e);
    assert_eq!(v["groups"]["lit"]["complete"], true);
    assert_eq!(v["groups"]["lit"]["optional_missing"], serde_json::json!(["s2-key"]));
    // a stored optional member is passed, a missing one is simply absent
    let ok = e.run(&["run", "lit", "--", "sh", "-c", "test \"$MUST\" = mmmmmmmm && test \"$OPENALEX_API_KEY\" = oooooooo && test -z \"${S2_API_KEY+x}\""], None);
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    // an optional name that is not one of the group's variables is a problem line
    assert!(v["problems"].to_string().contains("NOT_A_VAR"), "{}", v["problems"]);
}

#[test]
fn request_records_a_wanted_secret_without_a_value() {
    let e = Env::new();
    let o = e.run(&["request", "openreview-password", "--env", "OPENREVIEW_PASSWORD", "--reason", "log in to OpenReview",
                    "--obtain", "https://openreview.net", "--json"], None);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["name"], "openreview-password");
    assert_eq!(v["env"], "OPENREVIEW_PASSWORD");
    assert_eq!(v["stored"], false);
    assert_eq!(v["declared_by"], "requested");
    let file = e.home().join(".config/keyfob/declarations.d/requested.json");
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(file.parent().unwrap()), 0o700);
    // a second request merges; the first stays
    assert!(e.run(&["request", "s2-key", "--reason", "search"], None).status.success());
    let f: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(f["secrets"]["openreview-password"]["description"], "log in to OpenReview");
    assert_eq!(f["secrets"]["s2-key"]["env"], "S2_KEY");
    // ls shows what is wanted and why
    let ls: serde_json::Value = serde_json::from_str(&out(&e.run(&["ls", "--json"], None))).unwrap();
    let row = ls["secrets"].as_array().unwrap().iter().find(|s| s["name"] == "openreview-password").unwrap().clone();
    assert_eq!(row["stored"], false);
    assert_eq!(row["declared_by"], "requested");
    assert_eq!(row["obtain"], "https://openreview.net");
    // asking again replaces the reason
    assert!(e.run(&["request", "s2-key", "--reason", "search again"], None).status.success());
    let f: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(f["secrets"]["s2-key"]["description"], "search again");
    // a name a plugin already declares is not written again
    e.decl(DECL);
    let o = e.run(&["request", "demo-key", "--reason", "x", "--json"], None);
    assert!(o.status.success());
    let v: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(v["declared_by"], "demo-plugin");
    let f: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(f["secrets"]["demo-key"].is_null());
    // bad input is refused
    assert!(!e.run(&["request", "Bad Name", "--reason", "x"], None).status.success());
    assert!(!e.run(&["request", "ok-name", "--env", "1BAD", "--reason", "x"], None).status.success());
    assert!(!e.run(&["request", "ok-name"], None).status.success());
    assert!(!e.run(&["request", "ok-name", "--reason", "  "], None).status.success());
}
