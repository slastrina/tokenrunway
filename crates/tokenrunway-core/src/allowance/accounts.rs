//! The account list: `accounts.json` in the app data dir. No secrets in it: pasted keys live in
//! the OS keychain (see `secrets`), CLI logins stay where the CLI keeps them.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One account as saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountRecord {
    /// "acct-1a2b3c"
    pub id: String,
    /// "anthropic" | "openai" | "bedrock" | "anthropic-api" | "openai-api"
    pub provider: String,
    /// "cli" | "login" | "key" | "aws"
    pub method: String,
    /// "Claude — work"
    pub label: String,
    /// The plan as last read ("Max", "Business"), so a card has it before the first reading
    pub plan: String,
    /// The AWS profile, for "aws"
    pub profile: Option<String>,
    /// A monthly limit in USD the user set, for spend windows that have none of their own
    pub limit_usd: Option<f64>,
    pub created_at: i64,
    /// The CLI logins this account has read (Claude account uuids, ChatGPT account ids; no
    /// tokens), so the dashboard can tell its sessions from another login's
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub logins: Vec<String>,
}

/// The saved list, and where it lives.
#[derive(Debug)]
pub struct Accounts {
    path: PathBuf,
    pub list: Vec<AccountRecord>,
}

impl Accounts {
    /// Loads `<dir>/accounts.json` (an empty list if there's none or it can't be read).
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("accounts.json");
        let list = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { path, list }
    }

    pub fn save(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.list).map_err(|e| e.to_string())?;
        write_atomic(&self.path, &json).map_err(|e| format!("Couldn't save the accounts: {e}"))
    }

    pub fn get(&self, id: &str) -> Option<&AccountRecord> {
        self.list.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut AccountRecord> {
        self.list.iter_mut().find(|a| a.id == id)
    }

    /// A fresh id that isn't taken.
    pub fn new_id(&self, now: i64) -> String {
        let mut n = (now as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ std::process::id() as u64;
        loop {
            let id = format!("acct-{:06x}", n & 0xff_ffff);
            if self.get(&id).is_none() {
                return id;
            }
            n = n.wrapping_add(0x5851_F42D);
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<AccountRecord> {
        let i = self.list.iter().position(|a| a.id == id)?;
        Some(self.list.remove(i))
    }

    /// The other accounts reading the same CLI login as `id` (another Claude Code account).
    pub fn peers(&self, id: &str) -> Vec<&AccountRecord> {
        let Some(me) = self.get(id) else { return Vec::new() };
        self.list.iter().filter(|a| a.id != id && a.provider == me.provider && a.method == me.method).collect()
    }

    /// Notes that account `id` just read CLI login `login`; true if that changed the list.
    ///
    /// A new account takes the login it was added under (from whichever account had it). The
    /// only account for a CLI collects every login it reads, so one Claude account still counts
    /// all of Claude Code. With several, an account keeps the logins it had: the CLI being
    /// signed in as someone else doesn't make their sessions this account's.
    pub fn learn_login(&mut self, id: &str, login: &str) -> bool {
        let Some(me) = self.get(id) else { return false };
        if me.logins.iter().any(|l| l == login) {
            return false;
        }
        let fresh = me.logins.is_empty();
        let peers: Vec<String> = self.peers(id).iter().map(|a| a.id.clone()).collect();
        if !fresh && !peers.is_empty() {
            return false;
        }
        for p in &peers {
            if let Some(a) = self.get_mut(p) {
                a.logins.retain(|l| l != login);
            }
        }
        if let Some(me) = self.get_mut(id) {
            me.logins.push(login.to_string());
        }
        true
    }
}

/// Writes next to the file and renames over it, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("tokenrunway-accounts-{}", std::process::id()));
        let mut a = Accounts::load(&dir);
        assert!(a.list.is_empty());
        let id = a.new_id(1);
        a.list.push(AccountRecord {
            id: id.clone(),
            provider: "anthropic".into(),
            method: "cli".into(),
            label: "Claude".into(),
            ..Default::default()
        });
        a.save().unwrap();
        let b = Accounts::load(&dir);
        assert_eq!(b.list, a.list);
        assert_ne!(b.new_id(1), id, "ids aren't reused");
        let raw = std::fs::read_to_string(dir.join("accounts.json")).unwrap();
        assert!(!raw.contains("key\":\"sk-"), "no secrets");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logins_follow_the_account_they_were_added_under() {
        let rec = |id: &str| AccountRecord { id: id.into(), provider: "anthropic".into(), method: "cli".into(), ..Default::default() };
        let mut a = Accounts { path: PathBuf::new(), list: vec![rec("work")] };
        assert!(a.learn_login("work", "u-work"));
        assert!(!a.learn_login("work", "u-work"), "already known");
        // The only Claude account collects every login Claude Code is signed in as.
        assert!(a.learn_login("work", "u-home"));
        assert_eq!(a.get("work").unwrap().logins, ["u-work", "u-home"]);
        // A second account added while signed in as "home" takes that login over.
        a.list.push(rec("home"));
        assert!(a.learn_login("home", "u-home"));
        assert_eq!(a.get("work").unwrap().logins, ["u-work"]);
        assert_eq!(a.get("home").unwrap().logins, ["u-home"]);
        // Now both read whoever is signed in; neither picks up the other's login.
        assert!(!a.learn_login("work", "u-home"));
        assert!(!a.learn_login("home", "u-work"));
        // Another provider's account isn't a peer.
        a.list.push(AccountRecord { id: "gpt".into(), provider: "openai".into(), method: "cli".into(), ..Default::default() });
        assert!(a.peers("gpt").is_empty());
        assert!(a.learn_login("gpt", "acc-1") && a.learn_login("gpt", "acc-2"));
        // Saved without the field when empty, so older versions read the file as before.
        assert!(!serde_json::to_string(&rec("x")).unwrap().contains("logins"));
    }
}
