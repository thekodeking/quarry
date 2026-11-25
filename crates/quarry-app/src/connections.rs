// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Saved connection profiles: JSON in ~/.config/quarry, passwords in the OS keychain.

use crate::state::ConnectionProfile;
use std::path::PathBuf;

const KEYCHAIN_SERVICE: &str = "quarry";
/// macOS: one keychain item holds every password (id -> secret), so the Keychain asks once per
/// launch rather than once per connection. Windows and Linux don't prompt per item, and Windows
/// caps a credential at 2560 bytes, so there each connection keeps its own item.
const USE_VAULT: bool = cfg!(target_os = "macos");
const VAULT_ACCOUNT: &str = "vault";

type Vault = std::collections::HashMap<String, String>;

/// The vault, read from the keychain on first use and kept for the rest of the launch
static VAULT: std::sync::Mutex<Option<Vault>> = std::sync::Mutex::new(None);

fn load_vault_checked() -> Result<Vault, String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, VAULT_ACCOUNT)
        .map_err(|e| format!("Keychain unavailable: {e}"))?;
    match entry.get_password() {
        Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
        Err(keyring::Error::NoEntry) => Ok(Vault::default()),
        Err(e) => Err(format!(
            "Keychain access denied or failed ({e}). Allow Quarry in Keychain Access or re-enter your password."
        )),
    }
}

fn with_vault<R>(f: impl FnOnce(&mut Vault) -> R) -> R {
    let mut guard = VAULT.lock().unwrap_or_else(|e| e.into_inner());
    let vault = guard.get_or_insert_with(|| load_vault_checked().unwrap_or_default());
    f(vault)
}

fn try_with_vault<R>(f: impl FnOnce(&mut Vault) -> R) -> Result<R, String> {
    let mut guard = VAULT.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        // Do not cache a denied/failed Keychain read so subsequent Connect clicks can re-prompt.
        let loaded = load_vault_checked()?;
        *guard = Some(loaded);
    }
    Ok(f(guard.as_mut().unwrap()))
}

fn store_vault(vault: &Vault) -> Result<(), String> {
    let json = serde_json::to_string(vault).map_err(|e| e.to_string())?;
    keyring::Entry::new(KEYCHAIN_SERVICE, VAULT_ACCOUNT)
        .and_then(|e| e.set_password(&json))
        .map_err(|e| format!("keychain: {e}"))
}

/// One item per connection: Windows and Linux, and macOS secrets saved before the vault
fn legacy_secret(id: &str) -> Option<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, id)
        .and_then(|e| e.get_password())
        .ok()
}

fn delete_legacy_secret(id: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, id) {
        let _ = entry.delete_credential(); // nothing stored is fine
    }
}

/// `~/.config/quarry` (or `$XDG_CONFIG_HOME/quarry`); `%APPDATA%\quarry` on Windows
pub fn config_dir() -> Option<PathBuf> {
    static DIR: std::sync::LazyLock<Option<PathBuf>> = std::sync::LazyLock::new(|| {
        let unix_style = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|h| h.join(".config")))
            .map(|b| b.join("quarry"));
        if !cfg!(windows) {
            return unix_style;
        }
        let dir = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("quarry"));
        // Early Windows builds wrote to %USERPROFILE%\.config\quarry: move it over once
        if let (Some(old), Some(new)) = (&unix_style, &dir) {
            if old.is_dir() && !new.exists() {
                if let Err(e) = std::fs::rename(old, new) {
                    tracing::warn!("couldn't move config from {}: {e}", old.display());
                    return unix_style;
                }
            }
        }
        dir.or(unix_style)
    });
    DIR.clone()
}

fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("connections.json"))
}

pub fn load() -> Vec<ConnectionProfile> {
    let mut profiles: Vec<ConnectionProfile> = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if fix_default_names(&mut profiles) {
        let _ = save(&profiles);
    }
    profiles
}

/// The add-connection form used to pre-fill "Local SQLite", so non-SQLite connections were saved under that name.
/// Renames them to their address. Returns whether anything changed.
fn fix_default_names(profiles: &mut [ConnectionProfile]) -> bool {
    let mut changed = false;
    for p in profiles
        .iter_mut()
        .filter(|p| p.name == "Local SQLite" && p.driver != "sqlite")
    {
        p.name = display_address(p);
        changed = true;
    }
    changed
}

/// Writes every persistable profile. Passwords move to the keychain and never touch the file (C6).
pub fn save(profiles: &[ConnectionProfile]) -> Result<(), String> {
    let mut stored = Vec::new();
    let mut secrets = Vec::new();
    for p in profiles {
        let Some(driver) = p.driver() else {
            stored.push(p.clone()); // a driver this build lacks: keep the profile as it was
            continue;
        };
        if driver.is_ephemeral(&p.path_or_url) {
            continue;
        }
        let (path_or_url, secret) = driver.take_secret(&p.path_or_url);
        if let Some(secret) = secret {
            secrets.push((p.id.clone(), secret));
        }
        if let Some(ssh) = &p.ssh {
            if let Some(ssh_secret) = &ssh.secret {
                if !ssh_secret.is_empty() {
                    secrets.push((format!("{}:ssh", p.id), ssh_secret.clone()));
                }
            }
        }
        stored.push(ConnectionProfile {
            path_or_url,
            ..p.clone()
        });
    }
    if !USE_VAULT {
        for (id, secret) in &secrets {
            keyring::Entry::new(KEYCHAIN_SERVICE, id)
                .and_then(|e| e.set_password(secret))
                .map_err(|e| format!("keychain: {e}"))?;
        }
    }
    // Write the keychain only when a password actually changed: each write can prompt
    if USE_VAULT && !secrets.is_empty() {
        with_vault(|vault| {
            let mut changed = false;
            for (id, secret) in secrets {
                if vault.get(&id) != Some(&secret) {
                    vault.insert(id, secret);
                    changed = true;
                }
            }
            if changed {
                store_vault(vault)
            } else {
                Ok(())
            }
        })?;
    }
    let path = config_path().ok_or("no home directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&stored).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, json).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Where a connection points, without user or password (the driver decides the form)
pub fn display_address(p: &ConnectionProfile) -> String {
    p.driver().map_or_else(
        || p.path_or_url.clone(),
        |d| d.display_address(&p.path_or_url),
    )
}

pub fn forget_secret(id: &str) {
    let ssh_key = format!("{id}:ssh");
    if !USE_VAULT {
        delete_legacy_secret(id);
        delete_legacy_secret(&ssh_key);
        return;
    }
    with_vault(|vault| {
        let mut changed = false;
        if vault.remove(id).is_some() {
            changed = true;
        }
        if vault.remove(&ssh_key).is_some() {
            changed = true;
        }
        if changed {
            let _ = store_vault(vault);
        }
    });
    delete_legacy_secret(id);
    delete_legacy_secret(&ssh_key);
}

pub fn ssh_secret(id: &str) -> Option<String> {
    let key = format!("{id}:ssh");
    if !USE_VAULT {
        legacy_secret(&key)
    } else {
        with_vault(|vault| vault.get(&key).cloned().or_else(|| legacy_secret(&key)))
    }
}

/// Returns the connection string with its keychain password restored (if one is stored).
/// Exports connection profiles as sanitized JSON (passwords stripped) for sharing.
pub fn export_sanitized_profiles(profiles: &[ConnectionProfile]) -> Result<String, String> {
    let sanitized: Vec<ConnectionProfile> = profiles
        .iter()
        .map(|p| {
            let mut copy = p.clone();
            if let Some(driver) = copy.driver() {
                copy.path_or_url = driver.take_secret(&copy.path_or_url).0;
            }
            copy
        })
        .collect();
    serde_json::to_string_pretty(&sanitized).map_err(|e| e.to_string())
}

/// Imports connection profiles from sanitized JSON.
pub fn import_sanitized_profiles(json: &str) -> Result<Vec<ConnectionProfile>, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

pub fn with_secret(profile: &ConnectionProfile) -> String {
    try_with_secret(profile).unwrap_or_else(|_| profile.path_or_url.clone())
}

pub fn try_with_secret(profile: &ConnectionProfile) -> Result<String, String> {
    let Some(driver) = profile.driver() else {
        return Ok(profile.path_or_url.clone());
    };
    if driver.take_secret(&profile.path_or_url).1.is_some() {
        return Ok(profile.path_or_url.clone());
    }
    let secret = if !USE_VAULT {
        legacy_secret(&profile.id)
    } else {
        try_with_vault(|vault| {
            if let Some(s) = vault.get(&profile.id) {
                return Some(s.clone());
            }
            let s = legacy_secret(&profile.id)?;
            vault.insert(profile.id.clone(), s.clone());
            if store_vault(vault).is_ok() {
                delete_legacy_secret(&profile.id);
            }
            Some(s)
        })?
    };
    Ok(match secret {
        Some(secret) => driver.put_secret(&profile.path_or_url, &secret),
        None => profile.path_or_url.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::display_address;
    use crate::state::{ConnectionProfile, Environment};

    fn p(name: &str, driver: &str, url: &str) -> ConnectionProfile {
        ConnectionProfile {
            id: "x".into(),
            name: name.into(),
            driver: driver.into(),
            path_or_url: url.into(),
            env: Environment::Local,
            read_only: false,
            ssh: None,
            statement_timeout_secs: None,
            ssl: None,
        }
    }

    #[test]
    fn shows_where_a_connection_points() {
        assert_eq!(
            display_address(&p(
                "n",
                "postgres",
                "postgres://me:secret@localhost:5432/app?sslmode=disable"
            )),
            "localhost:5432/app"
        );
        assert_eq!(
            display_address(&p("n", "sqlite", "/tmp/data/app.db")),
            "app.db"
        );
    }

    #[test]
    fn renames_connections_saved_under_the_old_default() {
        let mut list = vec![
            p(
                "Local SQLite",
                "postgres",
                "postgres://postgres@localhost:5432/postgres",
            ),
            p("Local SQLite", "sqlite", "/tmp/a.db"),
        ];
        assert!(super::fix_default_names(&mut list));
        assert_eq!(
            (list[0].name.as_str(), list[1].name.as_str()),
            ("localhost:5432/postgres", "Local SQLite")
        );
        assert!(!super::fix_default_names(&mut list));
    }

    #[test]
    fn reads_profiles_saved_before_drivers() {
        let old = r#"[{"id":"a","name":"n","kind":"SqliteMemory","path_or_url":":memory:","env":"Local","read_only":false},
                      {"id":"b","name":"n","kind":"Postgres","path_or_url":"postgres://h/db","env":"Local","read_only":false}]"#;
        let list: Vec<ConnectionProfile> = serde_json::from_str(old).unwrap();
        assert_eq!(
            (list[0].driver.as_str(), list[1].driver.as_str()),
            ("sqlite", "postgres")
        );
        assert!(serde_json::to_string(&list[1])
            .unwrap()
            .contains(r#""driver":"postgres""#));
    }
}
