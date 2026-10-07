//! The subcommands.

use crate::decl::{self, Decls};
use crate::guard;
use crate::store::{self, Store};
use crate::util::{self, check_name, default_env, fail, fail_code, parse_opts, valid_env, Fail, Secret, R};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Read, Write};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const HELP: &str = "keyfob: API tokens for your shell, your scripts and Claude Code, kept out of everyone's way.

Values live in the OS keystore (the macOS Keychain, the Linux Secret Service) or, on a server with neither,
in one 0600 file in a 0700 folder. Nothing is exported into your shell: a command gets the secrets it asks
for, for its own run. Plugins say what they need in a keyfob.json of their own; keyfob finds those files.

  keyfob add <name> [--env VAR] [--note TEXT] [--from-env VAR | --stdin]
                                   store a secret; asks on the terminal without echo by default
  keyfob request <name> [--env VAR] --reason TEXT [--obtain URL] [--json]
                                   record that a secret is wanted (no value); /keyfob lists it to add
  keyfob run <group|name|name=VAR>... -- <command> [args]
                                   run a command with those secrets in its environment (alias: env)
  keyfob ls [--json]               what is stored or declared, where, who needs it; never values
  keyfob check [--live] [--json]   are the declared groups complete? --live asks each service
  keyfob get <name>                print a value (your terminal, your programs; not Claude's Bash)
  keyfob rm <name> [--yes]         delete a secret
  keyfob rename <old> <new>        give a secret another name
  keyfob which <ENV_VAR>           the group and secret behind an environment variable
  keyfob decls [--json]            the keyfob.json files found and what they declare
  keyfob doctor [--json]           storage in use, permissions, plaintext secrets left in rc files
  keyfob migrate --keychain-prefix <old/> [--names a,b] [--apply]
                                   copy Keychain items another tool kept under <old/><name>
  keyfob capture                   (for the Claude Code plugin) store tokens found in stdin, print it scrubbed
  keyfob scrub                     stdin to stdout with every stored value replaced by [keyfob:<name>]
  keyfob guard                     (for the Claude Code plugin) the PreToolUse hook for Bash

Storage: KEYFOB_BACKEND, or \"backend\" in ~/.config/keyfob/config.json: keychain, secret-service, file, auto.";

pub fn main(args: Vec<String>) -> i32 {
    let Some(cmd) = args.first() else {
        println!("{HELP}");
        return 0;
    };
    let rest = &args[1..];
    let r = match cmd.as_str() {
        "-h" | "--help" | "help" => {
            println!("{HELP}");
            Ok(())
        }
        "-V" | "--version" | "version" => {
            println!("keyfob {VERSION}");
            Ok(())
        }
        "add" | "set" => add(rest),
        "request" => request(rest),
        "get" => get(rest),
        "rm" | "delete" => rm(rest),
        "rename" | "mv" => rename(rest),
        "run" | "env" => run(rest),
        "ls" | "list" => ls(rest),
        "check" => check(rest),
        "which" => which(rest),
        "decls" => decls(rest),
        "doctor" => doctor(rest),
        "migrate" => migrate(rest),
        "capture" => capture(rest),
        "scrub" => scrub(rest),
        "patterns" => patterns(rest),
        "guard" => return guard::hook(),
        other => fail(format!("unknown command {other} (keyfob --help)")),
    };
    match r {
        Ok(()) => 0,
        Err(Fail { msg, code }) => {
            if !msg.is_empty() {
                eprintln!("keyfob: {msg}");
            }
            code
        }
    }
}

// ------------------------------------------------------------------ add / get / rm / rename
fn add(args: &[String]) -> R<()> {
    let Some(name) = args.first() else { return fail("usage: keyfob add <name> [--env VAR] [--note TEXT] [--from-env VAR | --stdin]") };
    check_name(name)?;
    let o = parse_opts(&args[1..], &[("--env", true), ("--note", true), ("--from-env", true), ("--stdin", false)])?;
    let value: Secret = if let Some(var) = o.get("--from-env") {
        match std::env::var(var) {
            Ok(v) if !v.is_empty() => Secret::new(v),
            _ => return fail(format!("${var} is not set in this environment")),
        }
    } else if o.contains_key("--stdin") {
        let mut s = Secret::new(String::new());
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        if !std::io::stdin().is_terminal() {
            return fail("no terminal to ask on: pipe the value with --stdin, or use --from-env VAR");
        }
        let a = Secret::new(rpassword::prompt_password(format!("value for {name} (hidden): "))?);
        let b = Secret::new(rpassword::prompt_password("again: ")?);
        if *a != *b {
            return fail("the two entries differ; nothing stored");
        }
        a
    };
    let value = Secret::new(value.trim().to_string());
    if value.is_empty() {
        return fail("empty value; nothing stored");
    }
    if value.contains('\n') {
        return fail("the value has a line break; a token is one line");
    }
    if let Some(env) = o.get("--env") {
        if !valid_env(env) {
            return fail(format!("'{env}' is not an environment variable name"));
        }
    }
    let st = store::open()?;
    st.put(name, &value)?;
    decl::index_put(
        name,
        &[("backend", Some(json!(st.name()))), ("env", o.get("--env").map(|e| json!(e))), ("note", o.get("--note").map(|n| json!(n))), ("length", Some(json!(value.chars().count())))],
    )?;
    println!("stored {name} ({} characters) in {}", value.chars().count(), st.name());
    Ok(())
}

// ------------------------------------------------------------------ request
const REQUESTED: &str = "requested";

/// Records a wanted secret in the person's own declaration file, so `ls` and /keyfob list it by name; no value
/// passes through here. A name another declaration already holds is left to that declaration.
fn request(args: &[String]) -> R<()> {
    let usage = "usage: keyfob request <name> [--env VAR] --reason TEXT [--obtain URL] [--json]";
    let Some(name) = args.first() else { return fail(usage) };
    check_name(name)?;
    let o = parse_opts(&args[1..], &[("--env", true), ("--reason", true), ("--obtain", true), ("--json", false)])?;
    let reason = o.get("--reason").map(|r| r.trim().to_string()).unwrap_or_default();
    if reason.is_empty() {
        return fail(format!("{usage}: say why it is wanted"));
    }
    if let Some(env) = o.get("--env") {
        if !valid_env(env) {
            return fail(format!("'{env}' is not an environment variable name"));
        }
    }
    let as_json = o.contains_key("--json");
    let d = decl::load();
    let stored = store::open()?.get(name)?.is_some();
    if let Some(sd) = d.secrets.get(name).filter(|s| s.source != REQUESTED) {
        if as_json {
            println!("{}", json!({ "name": name, "env": sd.env, "stored": stored, "declared_by": sd.source }));
        } else {
            println!("{name} is already declared by {} (${}){}", sd.source, sd.env, if stored { ", and stored" } else { "" });
        }
        return Ok(());
    }
    let env = o.get("--env").cloned().unwrap_or_else(|| default_env(name));
    let path = util::config_dir().join("declarations.d").join(format!("{REQUESTED}.json"));
    let mut doc = util::load_json(&path).filter(|v| v.is_object()).unwrap_or_else(|| json!({}));
    doc["name"] = json!(REQUESTED);
    if !doc["secrets"].is_object() {
        doc["secrets"] = json!({});
    }
    let mut entry = json!({ "env": env, "description": reason });
    if let Some(ob) = o.get("--obtain").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        entry["obtain"] = json!(ob);
    }
    doc["secrets"][name.as_str()] = entry;
    util::save_json(&path, &doc)?;
    if as_json {
        println!("{}", json!({ "name": name, "env": env, "stored": stored, "declared_by": REQUESTED }));
    } else {
        println!("requested {name} (${env}){}", if stored { "; it is already stored" } else { "" });
    }
    Ok(())
}

fn get(args: &[String]) -> R<()> {
    if args.len() != 1 {
        return fail("usage: keyfob get <name>");
    }
    check_name(&args[0])?;
    match store::open()?.get(&args[0])? {
        Some(v) => {
            let mut out = std::io::stdout();
            out.write_all(v.as_bytes())?;
            if out.is_terminal() {
                out.write_all(b"\n")?;
            }
            Ok(())
        }
        None => fail_code(1, format!("no secret named {} (keyfob ls)", args[0])),
    }
}

fn rm(args: &[String]) -> R<()> {
    let Some(name) = args.first() else { return fail("usage: keyfob rm <name> [--yes]") };
    check_name(name)?;
    let o = parse_opts(&args[1..], &[("--yes", false)])?;
    if !o.contains_key("--yes") {
        if !std::io::stdin().is_terminal() {
            return fail("no terminal to confirm on: add --yes");
        }
        eprint!("delete {name}? [y/N] ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            return fail_code(1, "kept");
        }
    }
    let gone = store::open()?.delete(name)?;
    decl::index_drop(name)?;
    println!("{}", if gone { format!("deleted {name}") } else { format!("{name} was not stored; its index entry is cleared") });
    Ok(())
}

fn rename(args: &[String]) -> R<()> {
    if args.len() != 2 {
        return fail("usage: keyfob rename <old> <new>");
    }
    let (old, new) = (&args[0], &args[1]);
    check_name(old)?;
    check_name(new)?;
    let st = store::open()?;
    let Some(v) = st.get(old)? else { return fail_code(1, format!("no secret named {old}")) };
    if st.get(new)?.is_some() {
        return fail(format!("{new} already exists; remove it first"));
    }
    st.put(new, &v)?;
    st.delete(old)?;
    let meta = decl::index().get(old).cloned().unwrap_or(Value::Null);
    decl::index_drop(old)?;
    decl::index_put(new, &[("backend", Some(json!(st.name()))), ("env", meta.get("env").cloned()), ("note", meta.get("note").cloned()), ("length", meta.get("length").cloned())])?;
    println!("renamed {old} to {new}");
    Ok(())
}

// ------------------------------------------------------------------ run
fn resolve(specs: &[String], d: &Decls) -> R<Vec<(String, String)>> {
    let mut pairs = vec![];
    for spec in specs {
        if let Some((name, var)) = spec.split_once('=') {
            check_name(name)?;
            if !valid_env(var) {
                return fail(format!("'{var}' is not an environment variable name"));
            }
            pairs.push((var.to_string(), name.to_string()));
        } else if let Some(g) = d.groups.get(spec) {
            pairs.extend(g.env.iter().map(|(v, s)| (v.clone(), s.clone())));
        } else {
            check_name(spec)?;
            pairs.push((decl::env_for(spec, d), spec.clone()));
        }
    }
    Ok(pairs)
}

/// The variables of the named groups that are optional: passed when stored, never required.
fn optional_vars(specs: &[String], d: &Decls) -> BTreeSet<String> {
    specs.iter().filter_map(|s| d.groups.get(s)).flat_map(|g| g.optional.iter().cloned()).collect()
}

fn run(args: &[String]) -> R<()> {
    let usage = "usage: keyfob run <group|name|name=VAR>... -- <command> [args]";
    let Some(cut) = args.iter().position(|a| a == "--") else { return fail(usage) };
    let (specs, command) = (&args[..cut], &args[cut + 1..]);
    if specs.is_empty() || command.is_empty() {
        return fail(usage);
    }
    let d = decl::load();
    let pairs = resolve(specs, &d)?;
    let optional = optional_vars(specs, &d);
    let st = store::open()?;
    let mut cmd = std::process::Command::new(&command[0]);
    cmd.args(&command[1..]);
    let mut missing = BTreeSet::new();
    for (var, name) in &pairs {
        match st.get(name)? {
            Some(v) => {
                cmd.env(var, v.as_str());
            }
            None if optional.contains(var) => {} // an optional member: the command runs without it
            None => {
                missing.insert(name.clone());
            }
        }
    }
    if !missing.is_empty() {
        let hints: Vec<String> = missing.iter().map(|m| match d.secrets.get(m).map(|s| s.obtain.as_str()).filter(|o| !o.is_empty()) {
            Some(o) => format!("{m} (get one at {o})"),
            None => m.clone(),
        }).collect();
        // old: ... Store each once: paste `keyfob: <name> <token>` into Claude Code, or run `keyfob add <name>` ...
        return fail_code(3, format!("missing {}. Store each once: in Claude Code, Claude asks with keyfob_request and /keyfob opens on it (or open /keyfob yourself); in a terminal, `keyfob add <name>`", hints.join(", ")));
    }
    let mut active: BTreeSet<String> = std::env::var("KEYFOB_ACTIVE").unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(String::from).collect();
    active.extend(specs.iter().filter(|s| d.groups.contains_key(*s)).cloned());
    cmd.env("KEYFOB_ACTIVE", active.into_iter().collect::<Vec<_>>().join(","));
    use std::os::unix::process::CommandExt;
    let err = cmd.exec();
    fail_code(127, format!("cannot run {}: {err}", command[0]))
}

// ------------------------------------------------------------------ ls / check / which / decls
struct Row {
    name: String,
    stored: bool,
    env: String,
    declared_by: String,
    description: String,
    obtain: String,
    note: String,
    updated: String,
}

fn collect(st: &dyn Store) -> R<(Decls, BTreeSet<String>, Vec<Row>)> {
    let d = decl::load();
    let idx = decl::index();
    let names: BTreeSet<String> = idx.keys().cloned().chain(d.secrets.keys().cloned()).collect();
    let mut have = BTreeSet::new();
    for n in &names {
        if st.get(n)?.is_some() {
            have.insert(n.clone());
        }
    }
    let rows = names
        .iter()
        .map(|n| {
            let sd = d.secrets.get(n);
            let i = idx.get(n).cloned().unwrap_or(Value::Null);
            Row {
                name: n.clone(),
                stored: have.contains(n),
                env: decl::env_for(n, &d),
                declared_by: sd.map(|s| s.source.clone()).unwrap_or_default(),
                description: sd.map(|s| s.description.clone()).unwrap_or_default(),
                obtain: sd.map(|s| s.obtain.clone()).unwrap_or_default(),
                note: i["note"].as_str().unwrap_or_default().to_string(),
                updated: i["updated"].as_str().unwrap_or_default().to_string(),
            }
        })
        .collect();
    Ok((d, have, rows))
}

fn group_status(d: &Decls, have: &BTreeSet<String>) -> Map<String, Value> {
    let mut out = Map::new();
    for (name, g) in &d.groups {
        // old: let missing: BTreeSet<&String> = g.env.values().filter(|s| !have.contains(*s)).collect();
        let missing: BTreeSet<&String> = g.env.iter().filter(|(v, s)| !g.optional.contains(*v) && !have.contains(*s)).map(|(_, s)| s).collect();
        let optional_missing: BTreeSet<&String> = g.env.iter().filter(|(v, s)| g.optional.contains(*v) && !have.contains(*s)).map(|(_, s)| s).collect();
        out.insert(
            name.clone(),
            json!({ "complete": missing.is_empty(), "missing": missing, "optional_missing": optional_missing, "env": g.env.keys().collect::<Vec<_>>(), "optional": g.optional, "source": g.source, "description": g.description }),
        );
    }
    out
}

fn ls(args: &[String]) -> R<()> {
    let o = parse_opts(args, &[("--json", false)])?;
    let st = store::open()?;
    let (d, have, rows) = collect(st.as_ref())?;
    if o.contains_key("--json") {
        let secrets: Vec<Value> = rows
            .iter()
            .map(|r| json!({ "name": r.name, "stored": r.stored, "env": r.env, "declared_by": r.declared_by, "description": r.description, "obtain": r.obtain, "note": r.note, "updated": r.updated }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&json!({ "backend": st.name(), "secrets": secrets, "groups": group_status(&d, &have) })).unwrap_or_default());
        return Ok(());
    }
    println!("storage: {}", st.name());
    if rows.is_empty() {
        println!("nothing stored or declared yet: keyfob add <name>");
    }
    for r in &rows {
        let who = if !r.declared_by.is_empty() { format!("needed by {}", r.declared_by) } else { r.note.clone() };
        println!("{} {:<26} {:<30} {}", if r.stored { "✓" } else { "✗" }, r.name, r.env, who);
    }
    Ok(())
}

fn wait_timeout(child: &mut std::process::Child, secs: u64) -> Option<std::process::ExitStatus> {
    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(s)) = child.try_wait() {
            return Some(s);
        }
        if start.elapsed().as_secs() >= secs {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn fill(template: &str, env: &BTreeMap<String, Secret>) -> String {
    let re = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();
    re.replace_all(template, |c: &regex::Captures| env.get(&c[1]).map(|s| s.to_string()).or_else(|| std::env::var(&c[1]).ok()).unwrap_or_default()).into_owned()
}

fn live_check(name: &str, g: &decl::Group, st: &dyn Store) -> R<Value> {
    if g.check.is_null() {
        return Ok(json!({ "group": name, "status": "no check declared" }));
    }
    let mut env: BTreeMap<String, Secret> = BTreeMap::new();
    for (var, s) in &g.env {
        match st.get(s)? {
            Some(v) => {
                env.insert(var.clone(), v);
            }
            None => return Ok(json!({ "group": name, "status": "incomplete" })),
        }
    }
    let t0 = std::time::Instant::now();
    let (status, detail): (&str, Value) = match g.check["type"].as_str() {
        Some("http") => {
            let url = fill(g.check["url"].as_str().unwrap_or_default(), &env);
            let config = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(std::time::Duration::from_secs(15))).build();
            let agent: ureq::Agent = config.into();
            let method = g.check["method"].as_str().unwrap_or("GET").to_uppercase();
            let mut req = ureq::http::Request::builder().method(method.as_str()).uri(url.as_str()).header("User-Agent", format!("keyfob/{VERSION}"));
            for (k, v) in g.check["headers"].as_object().into_iter().flatten() {
                req = req.header(k.as_str(), fill(v.as_str().unwrap_or_default(), &env));
            }
            if let Some([u, p]) = g.check["basic"].as_array().map(|a| a.as_slice()) {
                let pair = Secret::new(format!(
                    "{}:{}",
                    env.get(u.as_str().unwrap_or_default()).map(|s| s.as_str()).unwrap_or_default(),
                    env.get(p.as_str().unwrap_or_default()).map(|s| s.as_str()).unwrap_or_default()
                ));
                use base64::Engine;
                req = req.header("Authorization", format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(pair.as_bytes())));
            }
            match req.body(()).map_err(|e| e.to_string()).and_then(|r| agent.run(r).map_err(|e| e.to_string())) {
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    let ok: Vec<u64> = g.check["ok"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_else(|| vec![200]);
                    (if ok.contains(&(code as u64)) { "ok" } else if code == 401 || code == 403 { "rejected" } else { "error" }, json!(code))
                }
                Err(e) => ("unreachable", json!(e.chars().take(120).collect::<String>())),
            }
        }
        Some("command") => {
            let argv: Vec<String> = g.check["argv"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            if argv.is_empty() {
                ("bad check", json!("argv is empty"))
            } else {
                let mut c = std::process::Command::new(&argv[0]);
                c.args(&argv[1..]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null());
                for (k, v) in &env {
                    c.env(k, v.as_str());
                }
                match c.spawn() {
                    Ok(mut child) => match wait_timeout(&mut child, 30) {
                        Some(s) if s.success() => ("ok", json!(0)),
                        Some(s) => ("rejected", json!(s.code())),
                        None => ("unreachable", json!("timed out")),
                    },
                    Err(e) => ("unreachable", json!(e.to_string())),
                }
            }
        }
        other => ("bad check", json!(other)),
    };
    Ok(json!({ "group": name, "status": status, "detail": detail, "ms": t0.elapsed().as_millis() as u64 }))
}

fn check(args: &[String]) -> R<()> {
    let o = parse_opts(args, &[("--json", false), ("--live", false)])?;
    let st = store::open()?;
    let (d, have, _) = collect(st.as_ref())?;
    let groups = group_status(&d, &have);
    let mut live = vec![];
    if o.contains_key("--live") {
        for (n, g) in &d.groups {
            live.push(live_check(n, g, st.as_ref())?);
        }
    }
    let missing: BTreeSet<String> = groups.values().flat_map(|g| g["missing"].as_array().cloned().unwrap_or_default()).filter_map(|v| v.as_str().map(String::from)).collect();
    let rejected = live.iter().any(|l| l["status"] == "rejected");
    if o.contains_key("--json") {
        println!("{}", serde_json::to_string_pretty(&json!({ "backend": st.name(), "groups": groups, "missing": missing, "live": live, "problems": d.problems })).unwrap_or_default());
    } else {
        println!("storage: {}", st.name());
        if groups.is_empty() {
            println!("no groups declared (plugins declare them in keyfob.json; see keyfob decls)");
        }
        for (n, g) in &groups {
            let lv = live.iter().find(|l| l["group"] == n.as_str()).map(|l| format!("   live: {}", l["status"].as_str().unwrap_or("?"))).unwrap_or_default();
            let state = if g["complete"] == true { "complete".to_string() } else { format!("missing {}", g["missing"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()) };
            println!("{} {:<18} {}{}", if g["complete"] == true { "✓" } else { "✗" }, n, state, lv);
        }
        for p in &d.problems {
            println!("! {p}");
        }
    }
    if !missing.is_empty() || rejected {
        return fail_code(1, String::new()); // the report above says what; exit 1 for scripts
    }
    Ok(())
}

fn which(args: &[String]) -> R<()> {
    if args.len() != 1 {
        return fail("usage: keyfob which <ENV_VAR>");
    }
    let var = &args[0];
    let d = decl::load();
    let mut hits = vec![];
    for (gname, g) in &d.groups {
        if let Some(s) = g.env.get(var) {
            hits.push(format!("group {gname}, secret {s} (declared by {})", g.source));
        }
    }
    for (name, rec) in decl::index() {
        if rec["env"].as_str() == Some(var.as_str()) {
            hits.push(format!("secret {name} (stored with --env)"));
        }
    }
    if hits.is_empty() {
        return fail_code(1, format!("nothing provides ${var}"));
    }
    println!("{}", hits.join("\n"));
    Ok(())
}

fn decls(args: &[String]) -> R<()> {
    let o = parse_opts(args, &[("--json", false)])?;
    let d = decl::load();
    if o.contains_key("--json") {
        let groups: Map<String, Value> = d
            .groups
            .iter()
            .map(|(n, g)| (n.clone(), json!({ "env": g.env, "source": g.source, "description": g.description, "has_check": !g.check.is_null(), "require_for": g.require_for })))
            .collect();
        let secrets: Map<String, Value> = d
            .secrets
            .iter()
            .map(|(n, s)| (n.clone(), json!({ "env": s.env, "source": s.source, "description": s.description, "obtain": s.obtain, "has_pattern": !s.pattern.is_empty() })))
            .collect();
        let files: Vec<Value> = d.files.iter().map(|(p, n)| json!({ "path": p, "name": n })).collect();
        println!("{}", serde_json::to_string_pretty(&json!({ "files": files, "groups": groups, "secrets": secrets, "problems": d.problems })).unwrap_or_default());
        return Ok(());
    }
    if d.files.is_empty() {
        println!("no keyfob.json found (enabled plugins, ~/.config/keyfob/declarations.d, KEYFOB_DECLARATIONS)");
    }
    for (p, n) in &d.files {
        println!("{}  ({n})", util::tilde(p));
    }
    for (n, g) in &d.groups {
        println!("  group {:<16} {}", n, g.env.iter().map(|(v, s)| format!("{v}<-{s}")).collect::<Vec<_>>().join(", "));
    }
    for p in &d.problems {
        println!("! {p}");
    }
    Ok(())
}

// ------------------------------------------------------------------ doctor
const SHELL_FILES: &[&str] = &[".zshrc", ".zshenv", ".zprofile", ".bashrc", ".bash_profile", ".profile", ".env"];

fn plaintext_hits() -> Vec<Value> {
    let re = Regex::new(r#"(?m)^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*["']?([^\s"'#$][^"'#\s]{7,})"#).unwrap();
    let sourced = Regex::new(r#"(?m)(?:^|[;&|]\s*)(?:\.|source)\s+["']?((?:~|\$HOME|\$\{HOME\}|/)[^"'\s;&|)]+)"#).unwrap();
    let mut hits = vec![];
    // shell rc files, and the files they source (one level: a keys file loaded from .bashrc is the common case)
    let mut files: Vec<std::path::PathBuf> = SHELL_FILES.iter().map(|rel| util::home().join(rel)).collect();
    for f in files.clone() {
        if let Ok(text) = std::fs::read_to_string(&f) {
            for c in sourced.captures_iter(&text) {
                let raw = c[1].replace("${HOME}", "~").replace("$HOME", "~");
                let p = decl::expand_home(&raw);
                if p.is_file() && !files.contains(&p) {
                    files.push(p);
                }
            }
        }
    }
    for p in &files {
        let Ok(text) = std::fs::read_to_string(p) else { continue };
        for c in re.captures_iter(&text) {
            if util::secret_like_var(&c[1]) {
                let line = text[..c.get(0).unwrap().start()].matches('\n').count() + 1;
                hits.push(json!({ "file": util::tilde(p), "line": line, "var": &c[1] }));
            }
        }
    }
    // MCP servers in ~/.claude.json (user scope and per project): literal env values and credential headers
    if let Some(cj) = util::load_json(&util::home().join(".claude.json")) {
        let mut servers: Vec<(String, Value)> = cj["mcpServers"].as_object().into_iter().flatten().map(|(n, v)| (n.clone(), v.clone())).collect();
        for (proj, pv) in cj["projects"].as_object().into_iter().flatten() {
            for (n, v) in pv["mcpServers"].as_object().into_iter().flatten() {
                servers.push((format!("{n} (project {})", util::tilde(std::path::Path::new(proj))), v.clone()));
            }
        }
        for (name, cfg) in servers {
            for (k, v) in cfg["env"].as_object().into_iter().flatten() {
                if util::secret_like_var(k) && v.as_str().map(|s| s.len() >= 8 && !s.starts_with('$')).unwrap_or(false) {
                    hits.push(json!({ "file": format!("~/.claude.json mcpServers.{name}.env"), "line": null, "var": k }));
                }
            }
            for (k, v) in cfg["headers"].as_object().into_iter().flatten() {
                let credential = k.eq_ignore_ascii_case("authorization") || util::secret_like_var(&k.replace('-', "_"));
                if credential && v.as_str().map(|s| s.len() >= 16 && !s.contains("${")).unwrap_or(false) {
                    hits.push(json!({ "file": format!("~/.claude.json mcpServers.{name}.headers"), "line": null, "var": k }));
                }
            }
        }
    }
    for rel in ["settings.json", "settings.local.json"] {
        let p = util::claude_dir().join(rel);
        if let Some(env) = util::load_json(&p).and_then(|v| v["env"].as_object().cloned()) {
            for (var, val) in env {
                if util::secret_like_var(&var) && val.as_str().map(|s| s.len() >= 8 && !s.starts_with('$')).unwrap_or(false) {
                    hits.push(json!({ "file": util::tilde(&p), "line": null, "var": var }));
                }
            }
        }
    }
    hits
}

fn doctor(args: &[String]) -> R<()> {
    let o = parse_opts(args, &[("--json", false)])?;
    let backend = match store::open() {
        Ok(s) => s.name().to_string(),
        Err(e) => format!("unusable: {}", e.msg),
    };
    let backends: Vec<Value> = store::report().into_iter().map(|(n, ok, why)| json!({ "name": n, "usable": ok, "why": why })).collect();
    let mut perms = Map::new();
    for p in [util::config_dir(), util::data_dir(), util::secrets_file(), util::index_path()] {
        if let Some(m) = util::mode_of(&p) {
            perms.insert(util::tilde(&p), json!(format!("{m:o}")));
        }
    }
    let plain = plaintext_hits();
    let files = decl::files().len();
    if o.contains_key("--json") {
        println!("{}", serde_json::to_string_pretty(&json!({ "version": VERSION, "backend": backend, "backends": backends, "permissions": perms, "plaintext": plain, "declaration_files": files })).unwrap_or_default());
        return Ok(());
    }
    println!("keyfob {VERSION}, storage: {backend}");
    for b in &backends {
        println!("  {} {:<15} {}", if b["usable"] == true { "✓" } else { "·" }, b["name"].as_str().unwrap_or(""), b["why"].as_str().unwrap_or(""));
    }
    for (p, m) in &perms {
        println!("  {} {p}", m.as_str().unwrap_or(""));
    }
    println!("  {files} declaration file(s)");
    if plain.is_empty() {
        println!("no plaintext secrets in shell rc files or Claude settings");
    } else {
        println!("plaintext secrets to move in (keyfob add <name> --from-env VAR, then delete the line):");
        for h in &plain {
            let at = h["line"].as_u64().map(|l| format!(":{l}")).unwrap_or_default();
            println!("  {}{at}  {}", h["file"].as_str().unwrap_or(""), h["var"].as_str().unwrap_or(""));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ migrate
fn migrate(args: &[String]) -> R<()> {
    let o = parse_opts(args, &[("--keychain-prefix", true), ("--names", true), ("--apply", false)])?;
    let Some(prefix) = o.get("--keychain-prefix") else { return fail("usage: keyfob migrate --keychain-prefix <old/> [--names a,b] [--apply]") };
    let st = store::open()?;
    if st.name() != "keychain" {
        return fail(format!("migrate copies Keychain items; this machine stores in {}", st.name()));
    }
    let old = store::Keychain::new(&Value::Null);
    let names: Vec<String> = match o.get("--names") {
        Some(n) => n.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
        None => decl::load().secrets.keys().cloned().collect(),
    };
    for n in &names {
        check_name(n)?;
        match old.read_service(&format!("{prefix}{n}"))? {
            None => println!("· {n}: nothing under {prefix}{n}"),
            Some(v) if o.contains_key("--apply") => {
                st.put(n, &v)?;
                decl::index_put(n, &[("backend", Some(json!("keychain"))), ("note", Some(json!(format!("migrated from {prefix}")))), ("length", Some(json!(v.chars().count())))])?;
                println!("✓ {n} copied (the old item stays until you delete it in Keychain Access)");
            }
            Some(_) => println!("would copy {prefix}{n} -> keyfob/{n}"),
        }
    }
    if !o.contains_key("--apply") {
        println!("preview only: add --apply");
    }
    Ok(())
}

// ------------------------------------------------------------------ capture / scrub / patterns (for the plugin)
fn token_regexes(d: &Decls) -> Vec<(String, Regex)> {
    let mut v: Vec<(String, Regex)> = guard::TOKEN_PATTERNS.iter().filter_map(|p| Regex::new(p).ok().map(|r| (String::new(), r))).collect();
    for (name, s) in &d.secrets {
        if !s.pattern.is_empty() {
            if let Ok(r) = Regex::new(&s.pattern) {
                v.push((name.clone(), r));
            }
        }
    }
    v
}

fn patterns(args: &[String]) -> R<()> {
    let _ = parse_opts(args, &[("--json", false)])?;
    let d = decl::load();
    let list: Vec<Value> = token_regexes(&d).iter().map(|(n, r)| json!({ "secret": n, "regex": r.as_str() })).collect();
    println!("{}", json!({ "line": r"^\s*keyfob:\s*([a-z0-9][a-z0-9._-]{0,63})\s+(\S+)(?:\s+([A-Za-z_][A-Za-z0-9_]*))?\s*$", "tokens": list }));
    Ok(())
}

/// stdin: a message the user is sending. Stores `keyfob: <name> <token> [VAR]` lines and any recognisable token,
/// prints {text, stored, pending, failed} as JSON, where text has every value replaced. Never prints a value.
fn capture(args: &[String]) -> R<()> {
    let _ = parse_opts(args, &[])?;
    let mut text = Secret::new(String::new());
    std::io::stdin().read_to_string(&mut text)?;
    let d = decl::load();
    let line_re = Regex::new(r"(?m)^[ \t]*keyfob:[ \t]*([a-z0-9][a-z0-9._-]{0,63})[ \t]+(\S+)(?:[ \t]+([A-Za-z_][A-Za-z0-9_]*))?[ \t]*$").unwrap();
    let mut st: Option<Box<dyn Store>> = None;
    let (mut stored, mut pending, mut failed) = (vec![], vec![], vec![]);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for c in line_re.captures_iter(&text) {
        let m = c.get(0).unwrap();
        out.push_str(&text[last..m.start()]);
        last = m.end();
        let (name, value, var) = (c[1].to_string(), Secret::new(c[2].to_string()), c.get(3).map(|v| v.as_str().to_string()));
        if st.is_none() {
            st = Some(store::open()?);
        }
        let s = st.as_ref().unwrap();
        match s.put(&name, &value) {
            Ok(()) => {
                let env = var.clone().or_else(|| d.secrets.get(&name).map(|x| x.env.clone()));
                decl::index_put(&name, &[("backend", Some(json!(s.name()))), ("env", env.map(|e| json!(e))), ("note", Some(json!("pasted into Claude Code"))), ("length", Some(json!(value.chars().count())))])?;
                out.push_str(&format!("keyfob: {name} [stored in keyfob]"));
                stored.push(json!({ "name": name, "env": decl::env_for(&name, &d), "length": value.chars().count() }));
            }
            Err(e) => {
                out.push_str(&format!("keyfob: {name} [not stored: {}]", e.msg));
                failed.push(json!({ "name": name, "error": e.msg }));
            }
        }
    }
    out.push_str(&text[last..]);
    // loose tokens elsewhere in the message
    let regexes = token_regexes(&d);
    let mut text2 = Secret::new(out);
    for (declared, re) in &regexes {
        let found: BTreeSet<String> = re.find_iter(&text2).map(|m| m.as_str().to_string()).collect();
        for tok in found {
            let tok = Secret::new(tok);
            if text2.contains(&format!("[keyfob:{}]", tok.as_str())) {
                continue;
            }
            if st.is_none() {
                st = Some(store::open()?);
            }
            let s = st.as_ref().unwrap();
            let idx = decl::index();
            let name = if !declared.is_empty() && !idx.contains_key(declared) {
                declared.clone()
            } else {
                (1..).map(|n| format!("pasted-{n}")).find(|n| !idx.contains_key(n)).unwrap()
            };
            let replaced = match s.put(&name, &tok) {
                Ok(()) => {
                    decl::index_put(&name, &[("backend", Some(json!(s.name()))), ("note", Some(json!("found in a message to Claude Code"))), ("length", Some(json!(tok.chars().count())))])?;
                    pending.push(json!({ "name": name, "length": tok.chars().count(), "declared": !declared.is_empty() }));
                    format!("[keyfob:{name}]")
                }
                Err(e) => {
                    failed.push(json!({ "name": name, "error": e.msg }));
                    "[keyfob: a token was here; not stored]".to_string()
                }
            };
            *text2 = text2.replace(tok.as_str(), &replaced);
        }
    }
    println!("{}", json!({ "text": text2.as_str(), "stored": stored, "pending": pending, "failed": failed }));
    Ok(())
}

fn scrub(args: &[String]) -> R<()> {
    let _ = parse_opts(args, &[])?;
    let st = store::open()?;
    let mut text = Secret::new(String::new());
    std::io::stdin().read_to_string(&mut text)?;
    let mut vals: Vec<(Secret, String)> = vec![];
    for name in decl::index().keys() {
        if let Some(v) = st.get(name)? {
            if v.chars().count() >= 8 {
                vals.push((v, name.clone()));
            }
        }
    }
    vals.sort_by_key(|(v, _)| std::cmp::Reverse(v.len()));
    for (v, name) in &vals {
        if text.contains(v.as_str()) {
            *text = text.replace(v.as_str(), &format!("[keyfob:{name}]"));
        }
    }
    std::io::stdout().write_all(text.as_bytes())?;
    Ok(())
}
