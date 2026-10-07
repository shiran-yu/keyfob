//! The index (names only) and the declarations plugins make in their keyfob.json.

use crate::util::{self, default_env, valid_env, valid_name, R};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ index: what is stored, never a value
pub fn index() -> Map<String, Value> {
    util::load_json(&util::index_path()).and_then(|v| v["secrets"].as_object().cloned()).unwrap_or_default()
}

pub fn index_put(name: &str, meta: &[(&str, Option<Value>)]) -> R<()> {
    let mut idx = index();
    let mut rec = idx.get(name).and_then(|v| v.as_object().cloned()).unwrap_or_else(|| {
        let mut m = Map::new();
        m.insert("created".into(), json!(util::now()));
        m
    });
    for (k, v) in meta {
        if let Some(v) = v {
            rec.insert(k.to_string(), v.clone());
        }
    }
    rec.insert("updated".into(), json!(util::now()));
    idx.insert(name.to_string(), Value::Object(rec));
    util::save_json(&util::index_path(), &json!({ "version": 1, "secrets": idx }))
}

pub fn index_drop(name: &str) -> R<()> {
    let mut idx = index();
    if idx.remove(name).is_some() {
        util::save_json(&util::index_path(), &json!({ "version": 1, "secrets": idx }))?;
    }
    Ok(())
}

// ------------------------------------------------------------------ declarations
#[derive(Clone, Debug, Default)]
pub struct SecretDecl {
    pub env: String,
    pub description: String,
    pub obtain: String,
    pub pattern: String,
    pub source: String,
}

#[derive(Clone, Debug, Default)]
pub struct Group {
    pub env: BTreeMap<String, String>, // VAR -> secret name
    pub description: String,
    pub check: Value,
    pub require_for: Vec<String>, // regexes a Bash command matching needs the group injected
    pub source: String,
}

#[derive(Debug, Default)]
pub struct Decls {
    pub secrets: BTreeMap<String, SecretDecl>,
    pub groups: BTreeMap<String, Group>,
    pub deny_paths: Vec<PathBuf>,
    pub files: Vec<(PathBuf, String)>,
    pub problems: Vec<String>,
}

/// keyfob.json files from: ~/.config/keyfob/declarations.d/*.json, KEYFOB_DECLARATIONS (files or folders,
/// ':'-separated), and the root (or .claude-plugin/) of every enabled Claude Code plugin.
pub fn files() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    if let Ok(rd) = std::fs::read_dir(util::config_dir().join("declarations.d")) {
        let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().map(|x| x == "json").unwrap_or(false)).collect();
        v.sort();
        out.extend(v);
    }
    if let Some(extra) = std::env::var_os("KEYFOB_DECLARATIONS") {
        for p in std::env::split_paths(&extra) {
            out.push(if p.is_dir() { p.join("keyfob.json") } else { p });
        }
    }
    if std::env::var("KEYFOB_NO_PLUGINS").as_deref() != Ok("1") {
        let claude = util::claude_dir();
        let installed = util::load_json(&claude.join("plugins/installed_plugins.json")).unwrap_or(Value::Null);
        let enabled = util::load_json(&claude.join("settings.json")).map(|v| v["enabledPlugins"].clone()).unwrap_or(Value::Null);
        if let Some(plugins) = installed["plugins"].as_object() {
            for (key, entries) in plugins {
                if enabled[key.as_str()].as_bool() != Some(true) {
                    continue;
                }
                let list = entries.as_array().cloned().unwrap_or_else(|| vec![entries.clone()]);
                for e in list {
                    if let Some(root) = e["installPath"].as_str() {
                        for cand in [Path::new(root).join("keyfob.json"), Path::new(root).join(".claude-plugin/keyfob.json")] {
                            if cand.is_file() {
                                out.push(cand);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    out.into_iter().filter(|p| p.is_file() && seen.insert(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))).collect()
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

pub fn load() -> Decls {
    let mut d = Decls::default();
    for path in files() {
        let v = match util::load_json(&path) {
            Some(v) if v.is_object() => v,
            _ => {
                d.problems.push(format!("{}: not a JSON object", util::tilde(&path)));
                continue;
            }
        };
        let source = v["name"].as_str().map(String::from).unwrap_or_else(|| {
            path.parent().and_then(|p| if p.ends_with(".claude-plugin") { p.parent() } else { Some(p) }).and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        });
        d.files.push((path.clone(), source.clone()));
        if let Some(secrets) = v["secrets"].as_object() {
            for (name, sv) in secrets {
                if !valid_name(name) {
                    d.problems.push(format!("{}: bad secret name {name:?}", util::tilde(&path)));
                    continue;
                }
                let env = sv["env"].as_str().filter(|e| valid_env(e)).map(String::from).unwrap_or_else(|| default_env(name));
                d.secrets.entry(name.clone()).or_insert(SecretDecl {
                    env,
                    description: s(&sv["description"]),
                    obtain: s(&sv["obtain"]),
                    pattern: s(&sv["pattern"]),
                    source: source.clone(),
                });
            }
        }
        if let Some(groups) = v["groups"].as_object() {
            for (gname, gv) in groups {
                if !valid_name(gname) {
                    d.problems.push(format!("{}: bad group name {gname:?}", util::tilde(&path)));
                    continue;
                }
                if let Some(prev) = d.groups.get(gname) {
                    d.problems.push(format!("{}: group {gname} is already declared by {}", util::tilde(&path), prev.source));
                    continue;
                }
                let mut env = BTreeMap::new();
                for (var, sn) in gv["env"].as_object().into_iter().flatten() {
                    let sn = s(sn);
                    if valid_env(var) && valid_name(&sn) {
                        d.secrets.entry(sn.clone()).or_insert(SecretDecl { env: var.clone(), source: source.clone(), ..Default::default() });
                        env.insert(var.clone(), sn);
                    } else {
                        d.problems.push(format!("{}: group {gname}: bad entry {var} -> {sn}", util::tilde(&path)));
                    }
                }
                let require_for = gv["require_for"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                d.groups.insert(gname.clone(), Group { env, description: s(&gv["description"]), check: gv["check"].clone(), require_for, source: source.clone() });
            }
        }
        for p in v["guard"]["deny_paths"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
            d.deny_paths.push(expand_home(p));
        }
    }
    d
}

pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        util::home().join(rest)
    } else if p == "~" {
        util::home()
    } else {
        PathBuf::from(p)
    }
}

/// Every environment variable a declaration or the index ties to a secret.
pub fn secret_vars(d: &Decls) -> BTreeSet<String> {
    let mut v: BTreeSet<String> = d.secrets.values().map(|s| s.env.clone()).collect();
    for g in d.groups.values() {
        v.extend(g.env.keys().cloned());
    }
    for rec in index().values() {
        if let Some(e) = rec["env"].as_str() {
            v.insert(e.to_string());
        }
    }
    v
}

/// The variable a secret goes into by default: the one it was stored with, else its declaration's, else its name.
pub fn env_for(name: &str, d: &Decls) -> String {
    index().get(name).and_then(|r| r["env"].as_str().map(String::from)).or_else(|| d.secrets.get(name).map(|s| s.env.clone())).unwrap_or_else(|| default_env(name))
}
