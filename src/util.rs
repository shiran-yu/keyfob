//! Errors, paths, small file and encoding helpers shared by every command.

use base64::Engine;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// A secret value; zeroed when dropped.
pub type Secret = Zeroizing<String>;

/// A user-facing error: printed as `keyfob: <msg>`, exit status `code`.
#[derive(Debug)]
pub struct Fail {
    pub msg: String,
    pub code: i32,
}

pub type R<T> = Result<T, Fail>;

pub fn fail<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail { msg: msg.into(), code: 2 })
}

pub fn fail_code<T>(code: i32, msg: impl Into<String>) -> R<T> {
    Err(Fail { msg: msg.into(), code })
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail { msg: e.to_string(), code: 2 }
    }
}

// ------------------------------------------------------------------ paths (read on every call, so tests can steer them)
pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &[&str]) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => fallback.iter().fold(home(), |p, s| p.join(s)),
    }
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("KEYFOB_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| xdg("XDG_CONFIG_HOME", &[".config"]).join("keyfob"))
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("KEYFOB_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| xdg("XDG_DATA_HOME", &[".local", "share"]).join("keyfob"))
}

pub fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"))
}

pub fn index_path() -> PathBuf {
    config_dir().join("index.json")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn secrets_file() -> PathBuf {
    data_dir().join("secrets.json")
}

/// `~/x` for display.
pub fn tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

// ------------------------------------------------------------------ files
pub fn load_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Write JSON atomically with mode 0600 in a 0700 folder.
pub fn save_json(path: &Path, value: &Value) -> R<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    make_private_dir(dir)?;
    let tmp = dir.join(format!(".{}.tmp-{}", path.file_name().and_then(|s| s.to_str()).unwrap_or("x"), std::process::id()));
    let old = unsafe { libc::umask(0o077) };
    let result = (|| -> R<()> {
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
        f.write_all(serde_json::to_string_pretty(value).unwrap_or_default().as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    unsafe { libc::umask(old) };
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn make_private_dir(dir: &Path) -> R<()> {
    fs::create_dir_all(dir)?;
    let mode = fs::metadata(dir)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn mode_of(p: &Path) -> Option<u32> {
    fs::metadata(p).ok().map(|m| m.permissions().mode() & 0o777)
}

// ------------------------------------------------------------------ values
/// Stored form: `b64:<base64>`, so any byte survives every backend's quoting.
pub fn encode(value: &str) -> Secret {
    Zeroizing::new(format!("b64:{}", base64::engine::general_purpose::STANDARD.encode(value.as_bytes())))
}

pub fn decode(stored: &str) -> Option<Secret> {
    match stored.strip_prefix("b64:") {
        Some(b) => {
            let bytes = Zeroizing::new(base64::engine::general_purpose::STANDARD.decode(b.trim()).ok()?);
            String::from_utf8(bytes.to_vec()).ok().map(Zeroizing::new)
        }
        None => Some(Zeroizing::new(stored.to_string())),
    }
}

pub fn now() -> String {
    let t = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

pub fn user() -> String {
    let uid = unsafe { libc::getuid() };
    let pw = unsafe { libc::getpwuid(uid) };
    if !pw.is_null() {
        let name = unsafe { std::ffi::CStr::from_ptr((*pw).pw_name) };
        if let Ok(s) = name.to_str() {
            return s.to_string();
        }
    }
    std::env::var("USER").unwrap_or_else(|_| uid.to_string())
}

// ------------------------------------------------------------------ names
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

pub fn check_name(name: &str) -> R<()> {
    if valid_name(name) {
        Ok(())
    } else {
        fail(format!("'{name}' is not a secret name: lowercase letters, digits, '.', '_' and '-', up to 64"))
    }
}

pub fn valid_env(var: &str) -> bool {
    let b = var.as_bytes();
    !b.is_empty() && (b[0].is_ascii_alphabetic() || b[0] == b'_') && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// `t212-isa-key` -> `T212_ISA_KEY`.
pub fn default_env(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect()
}

/// Does a variable name look like it holds a secret? (`SSH_AUTH_SOCK`, `XAUTHORITY`, `OLDPWD`, `KEYTIMEOUT`: no.)
pub fn secret_like_var(var: &str) -> bool {
    let up = var.to_ascii_uppercase();
    ["SECRET", "TOKEN", "PASSWORD", "PASSWD", "CREDENTIAL", "API_KEY", "APIKEY", "ACCESS_KEY", "PRIVATE_KEY"].iter().any(|w| up.contains(w))
        || up.ends_with("_KEY")
        || up.ends_with("_PASS")
        || up == "KEY"
}

// ------------------------------------------------------------------ options
/// Long options only; `spec` lists (option, takes a value).
pub fn parse_opts(args: &[String], spec: &[(&str, bool)]) -> R<std::collections::BTreeMap<String, String>> {
    let mut out = std::collections::BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match spec.iter().find(|(o, _)| o == a) {
            None => return fail(format!("unknown option {a}")),
            Some((o, true)) => {
                let v = args.get(i + 1).ok_or_else(|| Fail { msg: format!("{o} needs a value"), code: 2 })?;
                out.insert(o.to_string(), v.clone());
                i += 2;
            }
            Some((o, false)) => {
                out.insert(o.to_string(), String::new());
                i += 1;
            }
        }
    }
    Ok(out)
}
