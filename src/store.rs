//! Where values live: the macOS Keychain, the freedesktop Secret Service, or one 0600 file.

use crate::util::{self, decode, encode, fail, Secret, R};
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

pub trait Store {
    fn name(&self) -> &'static str;
    fn get(&self, name: &str) -> R<Option<Secret>>;
    fn put(&self, name: &str, value: &str) -> R<()>;
    fn delete(&self, name: &str) -> R<bool>;
}

pub const SERVICE: &str = "keyfob";

/// Run a program, feeding `stdin`, returning (status, stdout, stderr). Values travel on pipes, never in argv.
fn pipe(program: &str, args: &[&str], stdin: Option<&str>) -> R<(i32, Secret, String)> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| util::Fail { msg: format!("cannot run {program}: {e}"), code: 2 })?;
    if let Some(input) = stdin {
        if let Some(mut s) = child.stdin.take() {
            s.write_all(input.as_bytes())?;
        }
    }
    let out = child.wait_with_output()?;
    let stdout = Secret::new(String::from_utf8_lossy(&out.stdout).into_owned());
    Ok((out.status.code().unwrap_or(-1), stdout, String::from_utf8_lossy(&out.stderr).into_owned()))
}

// ------------------------------------------------------------------ macOS Keychain
/// Generic passwords, service `keyfob/<name>`, account the user, through Apple's own `/usr/bin/security`:
/// its Keychain ACL stays the same across keyfob upgrades (no "allow access" dialog after each one), and it
/// reads items that other tools wrote with it. Writes go through `security -i` on stdin, so a value is never
/// in an argv another process could read.
pub struct Keychain {
    account: String,
    legacy: Vec<String>,
}

const SECURITY: &str = "/usr/bin/security";

impl Keychain {
    pub fn new(cfg: &Value) -> Self {
        let legacy = cfg["keychain_legacy_prefixes"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        Keychain { account: util::user(), legacy }
    }

    pub fn usable() -> (bool, String) {
        if !cfg!(target_os = "macos") {
            return (false, "not macOS".into());
        }
        if !std::path::Path::new(SECURITY).exists() {
            return (false, "no /usr/bin/security".into());
        }
        (true, String::new())
    }

    pub fn read_service(&self, service: &str) -> R<Option<Secret>> {
        let (code, out, _) = pipe(SECURITY, &["find-generic-password", "-a", &self.account, "-s", service, "-w"], None)?;
        if code != 0 {
            return Ok(None);
        }
        Ok(decode(out.trim_end_matches('\n')))
    }
}

impl Store for Keychain {
    fn name(&self) -> &'static str {
        "keychain"
    }

    fn get(&self, name: &str) -> R<Option<Secret>> {
        if let Some(v) = self.read_service(&format!("{SERVICE}/{name}"))? {
            return Ok(Some(v));
        }
        for prefix in &self.legacy {
            if let Some(v) = self.read_service(&format!("{prefix}{name}"))? {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }

    fn put(&self, name: &str, value: &str) -> R<()> {
        // Everything quoted here is our own: the account (a login name), the name (checked), base64.
        let cmd = Secret::new(format!(
            "add-generic-password -U -a \"{}\" -s \"{SERVICE}/{name}\" -l \"keyfob {name}\" -w \"{}\"\n",
            self.account.replace('"', ""),
            encode(value).as_str()
        ));
        let (code, _, err) = pipe(SECURITY, &["-i"], Some(&cmd))?;
        let back = self.read_service(&format!("{SERVICE}/{name}"))?;
        if code != 0 || back.as_deref().map(|s| s.as_str()) != Some(value) {
            return fail(format!(
                "the Keychain did not take the value ({}). Over SSH the login Keychain is locked: run `security unlock-keychain`, or use KEYFOB_BACKEND=file",
                err.trim().chars().take(160).collect::<String>()
            ));
        }
        Ok(())
    }

    fn delete(&self, name: &str) -> R<bool> {
        let (code, _, _) = pipe(SECURITY, &["delete-generic-password", "-a", &self.account, "-s", &format!("{SERVICE}/{name}")], None)?;
        Ok(code == 0)
    }
}

// ------------------------------------------------------------------ Secret Service (Linux desktops)
#[cfg(target_os = "linux")]
pub struct SecretServiceStore;

#[cfg(target_os = "linux")]
impl SecretServiceStore {
    pub fn usable() -> (bool, String) {
        // No session bus means no Secret Service (servers, clusters, plain SSH); do not wait for a timeout.
        let uid = unsafe { libc::getuid() };
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").map(|s| !s.is_empty()).unwrap_or(false)
            || std::path::Path::new(&format!("/run/user/{uid}/bus")).exists();
        if !bus {
            return (false, "no D-Bus session bus".into());
        }
        match secret_service::blocking::SecretService::connect(secret_service::EncryptionType::Dh) {
            Ok(ss) => match ss.get_default_collection() {
                Ok(_) => (true, String::new()),
                Err(e) => (false, format!("no default keyring: {e}")),
            },
            Err(e) => (false, format!("no Secret Service: {e}")),
        }
    }

    fn with<T>(f: impl FnOnce(&secret_service::blocking::Collection) -> Result<T, secret_service::Error>) -> R<T> {
        let ss = secret_service::blocking::SecretService::connect(secret_service::EncryptionType::Dh)
            .map_err(|e| util::Fail { msg: format!("Secret Service: {e}"), code: 2 })?;
        let coll = ss.get_default_collection().map_err(|e| util::Fail { msg: format!("Secret Service: {e}"), code: 2 })?;
        coll.ensure_unlocked().map_err(|e| util::Fail { msg: format!("the keyring is locked: {e}"), code: 2 })?;
        f(&coll).map_err(|e| util::Fail { msg: format!("Secret Service: {e}"), code: 2 })
    }
}

#[cfg(target_os = "linux")]
impl Store for SecretServiceStore {
    fn name(&self) -> &'static str {
        "secret-service"
    }

    fn get(&self, name: &str) -> R<Option<Secret>> {
        Self::with(|c| {
            let items = c.search_items(std::collections::HashMap::from([("service", SERVICE), ("name", name)]))?;
            match items.first() {
                Some(item) => {
                    let bytes = zeroize::Zeroizing::new(item.get_secret()?);
                    Ok(String::from_utf8(bytes.to_vec()).ok().and_then(|s| decode(&zeroize::Zeroizing::new(s))))
                }
                None => Ok(None),
            }
        })
    }

    fn put(&self, name: &str, value: &str) -> R<()> {
        let stored = encode(value);
        Self::with(|c| {
            c.create_item(&format!("keyfob {name}"), std::collections::HashMap::from([("service", SERVICE), ("name", name)]), stored.as_bytes(), true, "text/plain")
                .map(|_| ())
        })
    }

    fn delete(&self, name: &str) -> R<bool> {
        Self::with(|c| {
            let items = c.search_items(std::collections::HashMap::from([("service", SERVICE), ("name", name)]))?;
            let found = !items.is_empty();
            for item in items {
                item.delete()?;
            }
            Ok(found)
        })
    }
}

// ------------------------------------------------------------------ 0600 file (servers, clusters)
/// One JSON file, 0600 in a 0700 folder: what Claude Code itself uses for its own login on Linux.
pub struct FileStore;

impl FileStore {
    fn load(&self) -> R<serde_json::Map<String, Value>> {
        let path = util::secrets_file();
        if !path.exists() {
            return Ok(Default::default());
        }
        for (p, want) in [(util::data_dir(), 0o700), (path.clone(), 0o600)] {
            if let Some(mode) = util::mode_of(&p) {
                if mode & 0o077 != 0 {
                    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(want))?;
                    eprintln!("keyfob: {} was open to others ({mode:o}); set to {want:o}", util::tilde(&p));
                }
            }
        }
        Ok(util::load_json(&path).and_then(|v| v["secrets"].as_object().cloned()).unwrap_or_default())
    }

    fn save(&self, secrets: serde_json::Map<String, Value>) -> R<()> {
        util::save_json(&util::secrets_file(), &json!({ "version": 1, "secrets": secrets }))
    }
}

impl Store for FileStore {
    fn name(&self) -> &'static str {
        "file"
    }

    fn get(&self, name: &str) -> R<Option<Secret>> {
        Ok(self.load()?.get(name).and_then(|v| v.as_str()).and_then(decode))
    }

    fn put(&self, name: &str, value: &str) -> R<()> {
        let mut s = self.load()?;
        s.insert(name.to_string(), Value::String(encode(value).to_string()));
        self.save(s)
    }

    fn delete(&self, name: &str) -> R<bool> {
        let mut s = self.load()?;
        let found = s.remove(name).is_some();
        if found {
            self.save(s)?;
        }
        Ok(found)
    }
}

// ------------------------------------------------------------------ choosing one
pub fn config() -> Value {
    util::load_json(&util::config_path()).unwrap_or(Value::Null)
}

/// (name, usable, why not) for every backend this build knows.
pub fn report() -> Vec<(&'static str, bool, String)> {
    let mut v = vec![];
    let (ok, why) = Keychain::usable();
    v.push(("keychain", ok, why));
    #[cfg(target_os = "linux")]
    {
        let (ok, why) = SecretServiceStore::usable();
        v.push(("secret-service", ok, why));
    }
    #[cfg(not(target_os = "linux"))]
    v.push(("secret-service", false, "not Linux".to_string()));
    v.push(("file", true, String::new()));
    v
}

pub fn open() -> R<Box<dyn Store>> {
    let cfg = config();
    let want = std::env::var("KEYFOB_BACKEND").ok().filter(|s| !s.is_empty()).or_else(|| cfg["backend"].as_str().map(String::from)).unwrap_or_else(|| "auto".into());
    let pick = |name: &str| -> R<Option<Box<dyn Store>>> {
        Ok(match name {
            "keychain" if Keychain::usable().0 => Some(Box::new(Keychain::new(&cfg))),
            #[cfg(target_os = "linux")]
            "secret-service" if SecretServiceStore::usable().0 => Some(Box::new(SecretServiceStore)),
            "file" => Some(Box::new(FileStore)),
            _ => None,
        })
    };
    match want.as_str() {
        "auto" => {
            for name in ["keychain", "secret-service", "file"] {
                if let Some(s) = pick(name)? {
                    return Ok(s);
                }
            }
            fail("no usable storage")
        }
        "keychain" | "secret-service" | "file" => match pick(&want)? {
            Some(s) => Ok(s),
            None => {
                let why = report().into_iter().find(|(n, _, _)| *n == want).map(|(_, _, w)| w).unwrap_or_default();
                fail(format!("storage '{want}' is not usable here: {why}"))
            }
        },
        other => fail(format!("unknown storage '{other}': keychain, secret-service, file or auto")),
    }
}
