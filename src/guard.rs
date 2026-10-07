//! `keyfob guard`: a Claude Code PreToolUse hook for Bash. It splits the command the way a shell would
//! (quotes, pipes, `;`, `&&`, `$( )`, backticks, `bash -c`, heredocs) and judges each simple command, so a
//! word inside a quoted grep pattern is never mistaken for a command. It denies commands that would print a
//! secret into the conversation, read keyfob's own store, carry a literal token, or need a group's secrets
//! without `keyfob run <group> --`; it asks before `keyfob rm`. It fails open: on any error it says nothing.

use crate::decl::{self, Decls};
use crate::util;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Allow,
    Deny(String),
    Ask(String),
}

// ------------------------------------------------------------------ shell splitting
#[derive(Debug, Default, Clone)]
pub struct Simple {
    pub words: Vec<String>,
    pub redirects: Vec<String>, // redirection targets and here-string / heredoc bodies
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub commands: Vec<Simple>,
    pub nested: Vec<String>, // $( ), backticks: judged as commands of their own
}

#[allow(unused_assignments)] // the word/command macros reset state that the next character may not read
pub fn split(src: &str) -> Parsed {
    let mut p = Parsed::default();
    let chars: Vec<char> = src.chars().collect();
    let mut cur = Simple::default();
    let mut word = String::new();
    let mut in_word = false;
    let mut redirect_next = false;
    let mut pending_heredocs: Vec<(String, bool)> = vec![];
    let mut i = 0;

    macro_rules! end_word {
        () => {
            if in_word {
                if redirect_next {
                    cur.redirects.push(std::mem::take(&mut word));
                    redirect_next = false;
                } else {
                    cur.words.push(std::mem::take(&mut word));
                }
                in_word = false;
            }
        };
    }
    macro_rules! end_cmd {
        () => {
            end_word!();
            if !cur.words.is_empty() || !cur.redirects.is_empty() {
                p.commands.push(std::mem::take(&mut cur));
            }
            redirect_next = false;
        };
    }

    // `$(`: text up to the matching `)`, quotes respected; returns (content, index after it)
    fn balanced(chars: &[char], start: usize) -> (String, usize) {
        let (mut depth, mut i, mut out) = (1, start, String::new());
        let mut quote: Option<char> = None;
        while i < chars.len() {
            let c = chars[i];
            match quote {
                Some(q) => {
                    if c == '\\' && q == '"' && i + 1 < chars.len() {
                        out.push(c);
                        out.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    if c == q {
                        quote = None;
                    }
                }
                None => match c {
                    '\'' | '"' => quote = Some(c),
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            return (out, i + 1);
                        }
                    }
                    _ => {}
                },
            }
            out.push(c);
            i += 1;
        }
        (out, i)
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => {
                end_word!();
                i += 1;
            }
            '\n' => {
                end_cmd!();
                i += 1;
                // heredoc bodies start on the next line
                for (delim, strip) in std::mem::take(&mut pending_heredocs) {
                    let mut body = String::new();
                    while i < chars.len() {
                        let end = chars[i..].iter().position(|&x| x == '\n').map(|n| i + n).unwrap_or(chars.len());
                        let line: String = chars[i..end].iter().collect();
                        i = (end + 1).min(chars.len());
                        let cmp = if strip { line.trim_start_matches('\t') } else { line.as_str() };
                        if cmp == delim {
                            break;
                        }
                        body.push_str(&line);
                        body.push('\n');
                    }
                    if let Some(last) = p.commands.last_mut() {
                        let shell = last.words.first().map(|w| base(w)).map(|b| matches!(b, "bash" | "sh" | "zsh" | "dash" | "ksh")).unwrap_or(false);
                        if shell {
                            p.nested.push(body.clone());
                        }
                        last.redirects.push(body);
                    }
                }
            }
            ';' | '(' | ')' => {
                end_cmd!();
                i += 1;
            }
            '&' | '|' => {
                end_cmd!();
                i += 1;
                if i < chars.len() && (chars[i] == c || (c == '|' && chars[i] == '&') || (c == '&' && chars[i] == '>')) {
                    if c == '&' && chars[i] == '>' {
                        redirect_next = true;
                    }
                    i += 1;
                }
            }
            '<' | '>' => {
                // `2>` style: a word of digits right before is a file descriptor, not an argument
                if in_word && word.chars().all(|d| d.is_ascii_digit()) {
                    word.clear();
                    in_word = false;
                } else {
                    end_word!();
                }
                if c == '<' && chars.get(i + 1) == Some(&'<') && chars.get(i + 2) != Some(&'<') {
                    // heredoc: read the delimiter word
                    i += 2;
                    let strip = chars.get(i) == Some(&'-');
                    if strip {
                        i += 1;
                    }
                    while i < chars.len() && chars[i] == ' ' {
                        i += 1;
                    }
                    let mut delim = String::new();
                    while i < chars.len() && !chars[i].is_whitespace() && !";&|".contains(chars[i]) {
                        if chars[i] != '\'' && chars[i] != '"' {
                            delim.push(chars[i]);
                        }
                        i += 1;
                    }
                    pending_heredocs.push((delim, strip));
                    continue;
                }
                while i < chars.len() && (chars[i] == '<' || chars[i] == '>' || chars[i] == '&' || chars[i] == '|') {
                    i += 1;
                }
                redirect_next = true;
            }
            '#' if !in_word => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '\\' => {
                if let Some(&n) = chars.get(i + 1) {
                    if n != '\n' {
                        word.push(n);
                        in_word = true;
                    }
                }
                i += 2;
            }
            '\'' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
                i += 1;
            }
            '"' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() && "\"\\$`\n".contains(chars[i + 1]) {
                        if chars[i + 1] != '\n' {
                            word.push(chars[i + 1]);
                        }
                        i += 2;
                        continue;
                    }
                    if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
                        let (inner, next) = balanced(&chars, i + 2);
                        p.nested.push(inner.clone());
                        word.push_str("$(");
                        word.push_str(&inner);
                        word.push(')');
                        i = next;
                        continue;
                    }
                    if chars[i] == '`' {
                        let end = chars[i + 1..].iter().position(|&x| x == '`').map(|n| i + 1 + n).unwrap_or(chars.len());
                        p.nested.push(chars[i + 1..end].iter().collect());
                        i = end + 1;
                        continue;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
                i += 1;
            }
            '$' if chars.get(i + 1) == Some(&'(') => {
                let (inner, next) = balanced(&chars, i + 2);
                p.nested.push(inner.clone());
                word.push_str("$(");
                word.push_str(&inner);
                word.push(')');
                in_word = true;
                i = next;
            }
            '`' => {
                let end = chars[i + 1..].iter().position(|&x| x == '`').map(|n| i + 1 + n).unwrap_or(chars.len());
                p.nested.push(chars[i + 1..end].iter().collect());
                in_word = true;
                i = end + 1;
            }
            _ => {
                word.push(c);
                in_word = true;
                i += 1;
            }
        }
    }
    end_cmd!();
    p
}

fn base(w: &str) -> &str {
    w.rsplit('/').next().unwrap_or(w)
}

// ------------------------------------------------------------------ what the rules know
pub struct Ctx {
    pub secret_vars: BTreeSet<String>,
    pub protected: Vec<PathBuf>,
    pub patterns: Vec<Regex>,
    pub groups: Vec<GroupRule>,
}

pub struct GroupRule {
    pub name: String,
    pub secrets: BTreeSet<String>,
    pub vars: Vec<String>,
    pub complete: bool,
    pub require_for: Vec<Regex>,
}

/// High-confidence token formats; a declaration's own `pattern`s are added to these.
pub const TOKEN_PATTERNS: &[&str] = &[
    r"sk-ant-[A-Za-z0-9_-]{20,}",
    r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}",
    r"\bgh[pousr]_[A-Za-z0-9]{36,}",
    r"\bgithub_pat_[A-Za-z0-9_]{22,}",
    r"\bhf_[A-Za-z0-9]{30,}",
    r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
    r"\bAKIA[0-9A-Z]{16}\b",
    r"\bAIza[0-9A-Za-z_-]{35}\b",
    r"\b[rs]k_live_[0-9A-Za-z]{20,}",
    r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
];

impl Ctx {
    pub fn load() -> Ctx {
        let d: Decls = decl::load();
        let idx = decl::index();
        let mut protected = vec![util::data_dir()];
        protected.extend(d.deny_paths.iter().cloned());
        let mut patterns: Vec<Regex> = TOKEN_PATTERNS.iter().filter_map(|p| Regex::new(p).ok()).collect();
        patterns.extend(d.secrets.values().filter(|s| !s.pattern.is_empty()).filter_map(|s| Regex::new(&s.pattern).ok()));
        let groups = d
            .groups
            .iter()
            .map(|(name, g)| GroupRule {
                name: name.clone(),
                secrets: g.env.values().cloned().collect(),
                vars: g.env.keys().cloned().collect(),
                complete: g.env.values().all(|s| idx.contains_key(s)),
                require_for: g.require_for.iter().filter_map(|r| Regex::new(r).ok()).collect(),
            })
            .collect();
        Ctx { secret_vars: decl::secret_vars(&d), protected, patterns, groups }
    }

    fn is_secret_var(&self, var: &str) -> bool {
        self.secret_vars.contains(var) || util::secret_like_var(var)
    }

    fn mentions_secret_var(&self, text: &str) -> Option<String> {
        let re = Regex::new(r"\$\{?([A-Za-z_][A-Za-z0-9_]*)").unwrap();
        let vars: Vec<String> = re.captures_iter(text).map(|c| c[1].to_string()).collect();
        vars.into_iter().find(|v| self.is_secret_var(v))
    }

    fn literal_token(&self, text: &str) -> bool {
        self.patterns.iter().any(|r| r.is_match(text))
    }

    fn protected_path(&self, word: &str) -> Option<PathBuf> {
        let expanded = if let Some(rest) = word.strip_prefix("~/") {
            util::home().join(rest)
        } else if let Some(rest) = word.strip_prefix("$HOME/").or_else(|| word.strip_prefix("${HOME}/")) {
            util::home().join(rest)
        } else {
            PathBuf::from(word)
        };
        let abs = if expanded.is_absolute() { expanded } else { return None };
        self.protected.iter().find(|p| abs.starts_with(p) || Path::new(&abs) == p.as_path()).cloned()
    }
}

// ------------------------------------------------------------------ judging
const PREFIXES: &[&str] = &["sudo", "command", "builtin", "exec", "nohup", "time", "nice", "stdbuf", "caffeinate", "doas", "xargs"];
const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "fish"];
const SCRIPTERS: &[&str] = &["python", "python3", "node", "perl", "ruby", "deno", "bun"];
const PATH_OK: &[&str] = &["ls", "stat", "test", "[", "mkdir", "chmod", "du", "file"];

fn deny(s: impl Into<String>) -> Verdict {
    Verdict::Deny(format!("[keyfob] {}", s.into()))
}

fn assignment(w: &str) -> Option<(&str, &str)> {
    let (k, v) = w.split_once('=')?;
    if util::valid_env(k) {
        Some((k, v))
    } else {
        None
    }
}

/// Judge one simple command. `wrapped` collects the groups and secrets a `keyfob run` in front named.
fn judge_simple(sc: &Simple, ctx: &Ctx, wrapped: &mut BTreeSet<String>, nested: &mut Vec<String>, whole: &str) -> Verdict {
    let mut w: Vec<String> = sc.words.clone();
    // every word and redirect: literal tokens and protected paths
    for x in w.iter().chain(sc.redirects.iter()) {
        if ctx.literal_token(x) {
            return deny("the command carries a literal token. Store it once (the user pastes `keyfob: <name> <token>` into Claude Code, or runs `keyfob add <name>` in their terminal), then run the command under `keyfob run <name> -- ...` and read it from the environment.");
        }
    }
    // leading assignments
    while let Some((k, v)) = w.first().and_then(|x| assignment(x)).map(|(k, v)| (k.to_string(), v.to_string())) {
        if ctx.is_secret_var(&k) && v.len() >= 8 && !v.starts_with('$') {
            return deny(format!("${k} is set to a literal value on the command line. Keep it in keyfob and run the command under `keyfob run` instead."));
        }
        w.remove(0);
    }
    // wrappers in front of the real command
    loop {
        let Some(first) = w.first() else { return Verdict::Allow };
        let b = base(first).to_string();
        if PREFIXES.contains(&b.as_str()) {
            w.remove(0);
            while w.first().map(|x| x.starts_with('-')).unwrap_or(false) {
                w.remove(0);
            }
            continue;
        }
        if b == "timeout" {
            w.remove(0);
            while w.first().map(|x| x.starts_with('-')).unwrap_or(false) {
                w.remove(0);
            }
            if !w.is_empty() {
                w.remove(0); // the duration
            }
            continue;
        }
        if b == "env" {
            let mut rest: Vec<String> = w[1..].to_vec();
            loop {
                match rest.first().map(|s| s.as_str()) {
                    Some("-u") | Some("--unset") | Some("-C") | Some("--chdir") => {
                        rest.drain(..2.min(rest.len()));
                    }
                    Some(o) if o.starts_with('-') => {
                        rest.remove(0);
                    }
                    Some(a) if assignment(a).is_some() => {
                        let (k, v) = assignment(a).unwrap();
                        if ctx.is_secret_var(k) && v.len() >= 8 && !v.starts_with('$') {
                            return deny(format!("${k} is set to a literal value on the command line. Keep it in keyfob and use `keyfob run`."));
                        }
                        rest.remove(0);
                    }
                    _ => break,
                }
            }
            if rest.is_empty() {
                return deny("`env` with no command prints every environment variable, secrets included. To test one: `[ -n \"$VAR\" ] && echo set`.");
            }
            w = rest;
            continue;
        }
        if (b == "keyfob" || b == "vault") && matches!(w.get(1).map(|s| s.as_str()), Some("run") | Some("env")) {
            if let Some(cut) = w.iter().position(|x| x == "--") {
                for spec in &w[2..cut] {
                    wrapped.insert(spec.split('=').next().unwrap_or(spec).to_string());
                }
                w = w[cut + 1..].to_vec();
                continue;
            }
            return Verdict::Allow; // malformed; keyfob itself will refuse it
        }
        break;
    }
    let b = base(&w[0]).to_string();
    let args: Vec<&str> = w[1..].iter().map(|s| s.as_str()).collect();
    let sub = args.first().copied().unwrap_or("");

    match b.as_str() {
        "keyfob" | "vault" => match sub {
            "get" => return deny("`keyfob get` prints a secret into the conversation. Run the command that needs it under `keyfob run <group|name> -- <command>`; `keyfob ls` shows what is stored."),
            "rm" | "delete" => return Verdict::Ask(format!("[keyfob] Delete the secret {}? It cannot be undone.", args.get(1).unwrap_or(&"?"))),
            "migrate" if args.contains(&"--apply") => return Verdict::Ask("[keyfob] Copy these Keychain items into keyfob?".into()),
            "add" | "set" => {
                if args.contains(&"--stdin") {
                    let fed = Regex::new(r"(^|[;&|(]\s*)(echo|printf)\b|<<<").unwrap();
                    if fed.is_match(whole) {
                        return deny("the value would sit in this command, and so in the transcript. Ask the user to paste it into Claude Code as `keyfob: <name> <token>`, or to run `keyfob add <name>` in their own terminal.");
                    }
                } else if !args.contains(&"--from-env") {
                    return deny("`keyfob add` asks for the value on a terminal Claude does not have. Ask the user to paste it into Claude Code as `keyfob: <name> <token>` (it is stored before the message is kept), or to run `keyfob add <name>` in their own terminal.");
                }
            }
            _ => {}
        },
        "security" => {
            let reads = args.iter().any(|a| matches!(*a, "find-generic-password" | "find-internet-password")) && args.iter().any(|a| *a == "-w" || *a == "-g" || a.starts_with("-w") || a.starts_with("-g"));
            if reads || matches!(sub, "dump-keychain" | "export" | "-i") {
                return deny("this would print Keychain secrets into the conversation. Use `keyfob run <group|name> -- <command>` to hand a secret to the command that needs it.");
            }
        }
        "secret-tool" if sub == "lookup" => return deny("this would print a stored secret into the conversation. Use `keyfob run <group|name> -- <command>`."),
        "printenv" => {
            if args.iter().all(|a| a.starts_with('-')) || args.iter().any(|a| ctx.is_secret_var(a)) {
                return deny("this prints secret environment variables. To test one: `[ -n \"$VAR\" ] && echo set`.");
            }
        }
        "set" | "export" | "declare" | "typeset" => {
            let names: Vec<&&str> = args.iter().filter(|a| !a.starts_with('-') && !a.starts_with('+')).collect();
            if names.is_empty() && (args.is_empty() || args.iter().any(|a| matches!(*a, "-p" | "-x" | "-px" | "-xp"))) {
                return deny("this lists every variable, secrets included. To test one: `[ -n \"$VAR\" ] && echo set`.");
            }
        }
        "echo" | "printf" | "print" | "cat" => {
            for a in &args {
                if let Some(v) = ctx.mentions_secret_var(a) {
                    return deny(format!("this prints ${v}. To test it: `[ -n \"${v}\" ] && echo set`."));
                }
            }
        }
        _ => {}
    }
    if SHELLS.contains(&b.as_str()) {
        if let Some(pos) = args.iter().position(|a| a.starts_with('-') && a.contains('c')) {
            if let Some(script) = args.get(pos + 1) {
                nested.push(script.to_string());
            }
        }
    }
    if b == "eval" {
        nested.push(args.join(" "));
    }
    if SCRIPTERS.iter().any(|s| b == *s || b.starts_with(&format!("{s}3.")) || b.starts_with("python3")) {
        if let Some(pos) = args.iter().position(|a| matches!(*a, "-c" | "-e" | "--eval" | "-E")) {
            let code = args.get(pos + 1).copied().unwrap_or("");
            let dump = Regex::new(r"(print|console\.log|puts|p|pp|say)\s*\(?\s*(dict\(|Object\.entries\(|JSON\.stringify\()?\s*(os\.environ|process\.env|ENV|%ENV)\s*\)*\s*(\)|;|$)").unwrap();
            if dump.is_match(code) {
                return deny("this prints the whole environment, secrets included.");
            }
            let printy = Regex::new(r"\b(print|console\.log|puts|say|write)\b").unwrap();
            if printy.is_match(code) && ctx.secret_vars.iter().any(|v| code.contains(v.as_str())) {
                return deny("this script prints a secret environment variable.");
            }
        }
    }
    if b == "curl" || b == "wget" || b == "http" || b == "xh" {
        let user_pass = Regex::new(r"^[A-Za-z0-9._-]{12,}:[A-Za-z0-9._/+=-]{12,}$").unwrap();
        let bearer = Regex::new(r"(?i)authorization:\s*(bearer|token|basic)\s+[A-Za-z0-9._/+=-]{16,}").unwrap();
        for (i, a) in args.iter().enumerate() {
            let val = if *a == "-u" || *a == "--user" || *a == "-H" || *a == "--header" { args.get(i + 1).copied() } else { None };
            if let Some(v) = val {
                if user_pass.is_match(v) || bearer.is_match(v) {
                    return deny("the request carries a literal credential. Read it from the environment and run under `keyfob run`.");
                }
            }
        }
    }
    if !PATH_OK.contains(&b.as_str()) {
        for x in w.iter().skip(1).chain(sc.redirects.iter()) {
            if let Some(p) = ctx.protected_path(x) {
                return deny(format!("{} holds secrets; reading it would put them in the conversation. Use `keyfob ls` and `keyfob run`.", util::tilde(&p)));
            }
        }
    }
    Verdict::Allow
}

pub fn judge(cmd: &str, ctx: &Ctx) -> Verdict {
    judge_depth(cmd, ctx, 0)
}

fn judge_depth(cmd: &str, ctx: &Ctx, depth: usize) -> Verdict {
    if depth > 4 {
        return Verdict::Allow;
    }
    let parsed = split(cmd);
    let mut wrapped = BTreeSet::new();
    let mut nested = parsed.nested.clone();
    let mut ask = None;
    for sc in &parsed.commands {
        match judge_simple(sc, ctx, &mut wrapped, &mut nested, cmd) {
            Verdict::Allow => {}
            Verdict::Ask(r) => ask = ask.or(Some(r)),
            d => return d,
        }
    }
    for inner in nested {
        match judge_depth(&inner, ctx, depth + 1) {
            Verdict::Allow => {}
            Verdict::Ask(r) => ask = ask.or(Some(r)),
            d => return d,
        }
    }
    // a command a group's declaration says needs its secrets, not run under `keyfob run <group>`
    if depth == 0 {
        for g in &ctx.groups {
            if !g.complete || !g.require_for.iter().any(|r| r.is_match(cmd)) {
                continue;
            }
            let covered = wrapped.contains(&g.name) || g.secrets.iter().all(|s| wrapped.contains(s)) || g.vars.iter().all(|v| std::env::var_os(v).is_some());
            if !covered {
                return deny(format!("this command needs the {0} secrets, which are in keyfob, not in the environment. Run it as: keyfob run {0} -- <the same command>", g.name));
            }
        }
    }
    ask.map(Verdict::Ask).unwrap_or(Verdict::Allow)
}

/// The hook: Claude Code's PreToolUse JSON on stdin, a decision on stdout (nothing means allow).
pub fn hook() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let v: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    if v["tool_name"].as_str().map(|t| t != "Bash").unwrap_or(false) {
        return 0;
    }
    let cmd = v["tool_input"]["command"].as_str().unwrap_or("");
    if cmd.trim().is_empty() {
        return 0;
    }
    let (decision, reason) = match judge(cmd, &Ctx::load()) {
        Verdict::Allow => return 0,
        Verdict::Deny(r) => ("deny", r),
        Verdict::Ask(r) => ("ask", r),
    };
    println!("{}", json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": decision, "permissionDecisionReason": reason } }));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            secret_vars: ["BINANCE_API_KEY", "BINANCE_SECRET_KEY", "MINERU_TOKEN"].iter().map(|s| s.to_string()).collect(),
            protected: vec![PathBuf::from("/home/u/.local/share/keyfob"), util::home().join(".binance")],
            patterns: TOKEN_PATTERNS.iter().map(|p| Regex::new(p).unwrap()).collect(),
            groups: vec![GroupRule {
                name: "binance".into(),
                secrets: ["binance-key", "binance-secret"].iter().map(|s| s.to_string()).collect(),
                vars: vec!["BINANCE_API_KEY".into(), "BINANCE_SECRET_KEY".into()],
                complete: true,
                require_for: vec![Regex::new(r"(^|[\s;&|(/])binance-cli\b").unwrap()],
            }],
        }
    }

    fn allowed(c: &str) -> bool {
        judge(c, &ctx()) == Verdict::Allow
    }
    fn denied(c: &str) -> bool {
        matches!(judge(c, &ctx()), Verdict::Deny(_))
    }

    #[test]
    fn words_inside_quotes_are_not_commands() {
        // both of these were refused by the old regex guard
        assert!(allowed(r#"grep -n -E "^\s+[a-z]+[:?(]|stdin|env|argv" types.d.ts"#));
        assert!(allowed(r#"grep -rn -E "vault (env|get)|trading-vault" plugins"#));
        assert!(allowed("echo 'keyfob get x is how you would do it' > notes.md"));
    }

    #[test]
    fn printing_secrets_is_denied() {
        assert!(denied("keyfob get binance-key"));
        assert!(denied("cd /tmp && keyfob get x | pbcopy"));
        assert!(denied("echo $(keyfob get x)"));
        assert!(denied("bash -c 'keyfob get x'"));
        assert!(denied("env"));
        assert!(denied("env | grep KEY"));
        assert!(denied("printenv"));
        assert!(denied("printenv MINERU_TOKEN"));
        assert!(denied("echo $MINERU_TOKEN"));
        assert!(denied("printf '%s' \"${BINANCE_SECRET_KEY}\""));
        assert!(denied("echo $GITHUB_TOKEN"));
        assert!(denied("export -p"));
        assert!(denied("set"));
        assert!(denied("security find-generic-password -s keyfob/x -w"));
        assert!(denied("secret-tool lookup service keyfob name x"));
        assert!(denied("python3 -c 'import os; print(os.environ)'"));
        assert!(denied("node -e 'console.log(process.env)'"));
        assert!(denied("python3 -c \"import os;print(os.environ['MINERU_TOKEN'])\""));
    }

    #[test]
    fn harmless_env_use_is_allowed() {
        assert!(allowed("env FOO=1 make test"));
        assert!(allowed("[ -n \"$MINERU_TOKEN\" ] && echo set"));
        assert!(allowed("printenv HOME"));
        assert!(allowed("echo $HOME $PATH"));
        assert!(allowed("export PATH=$PATH:/opt/bin"));
        assert!(allowed("python3 -c \"import os,re;print(sorted(k for k in os.environ if re.search('KEY',k)))\""));
        assert!(allowed("keyfob ls --json"));
        assert!(allowed("keyfob add mineru-token --from-env MINERU_TOKEN"));
    }

    #[test]
    fn literal_tokens_are_denied() {
        assert!(denied("curl -H 'Authorization: Bearer abcdefghijklmnopqrstuvwxyz123' https://api.x"));
        assert!(denied("curl -u AAAAAAAAAAAAAAAA:BBBBBBBBBBBBBBBB https://live.trading212.com"));
        assert!(denied("MINERU_TOKEN=eyJhbGciOiJIUzI1NiJ9xyz python3 run.py"));
        assert!(denied("git clone https://ghp_abcdefghijklmnopqrstuvwxyz0123456789AB@github.com/x/y"));
        assert!(denied("echo sk-ant-api03-abcdefghijklmnopqrstuvwxyz | keyfob add a --stdin"));
        assert!(denied("keyfob add anthropic --stdin <<< 'abc'"));
        assert!(denied("keyfob add anthropic"));
    }

    #[test]
    fn protected_paths_are_denied() {
        assert!(denied("cat /home/u/.local/share/keyfob/secrets.json"));
        assert!(denied("grep api ~/.binance/main"));
        assert!(denied("python3 x.py < /home/u/.local/share/keyfob/secrets.json"));
        assert!(allowed("ls -la /home/u/.local/share/keyfob"));
    }

    #[test]
    fn group_commands_need_keyfob_run() {
        if std::env::var_os("BINANCE_API_KEY").is_some() {
            return;
        }
        assert!(denied("binance-cli futures-usds futures-account-balance-v3"));
        assert!(denied("cd x && /opt/bin/binance-cli spot ping"));
        assert!(allowed("keyfob run binance -- binance-cli futures-usds futures-account-balance-v3"));
        assert!(allowed("vault env binance -- binance-cli spot ping"));
        assert!(allowed("keyfob run binance-key binance-secret -- binance-cli spot ping"));
        // printing through a wrapped command is still printing
        assert!(denied("keyfob run binance -- sh -c 'echo $BINANCE_API_KEY'"));
    }

    #[test]
    fn deletion_asks() {
        assert!(matches!(judge("keyfob rm old-token", &ctx()), Verdict::Ask(_)));
    }

    #[test]
    fn heredoc_bodies_are_checked() {
        assert!(denied("bash <<'EOF'\nkeyfob get x\nEOF"));
        assert!(denied("cat > run.sh <<EOF\nexport MINERU_TOKEN=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijkl\nEOF"));
        assert!(allowed("cat > notes.md <<EOF\nuse keyfob run\nEOF"));
    }
}
