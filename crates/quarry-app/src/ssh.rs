// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! SSH tunnels (C4, UC2) via the system `ssh` client: agent or key file, no prompts.

use serde::{Deserialize, Serialize};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SshAuthMode {
    #[default]
    Agent,
    Key,
    Password,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub auth_mode: SshAuthMode,
    /// Private key file; used for Key mode
    #[serde(default)]
    pub key_path: Option<String>,
    /// Optional jump host / bastion hop (-J jump.example.com)
    #[serde(default)]
    pub jump_host: Option<String>,
    /// Password or key passphrase; not saved to connections.json (saved to Keychain)
    #[serde(skip)]
    pub secret: Option<String>,
}

impl SshConfig {
    pub fn effective_auth_mode(&self) -> SshAuthMode {
        if self.auth_mode == SshAuthMode::Agent
            && self
                .key_path
                .as_ref()
                .is_some_and(|k| !k.trim().is_empty())
        {
            SshAuthMode::Key
        } else {
            self.auth_mode
        }
    }
}

/// A running `ssh -L`; the process is stopped when this is dropped
pub struct Tunnel {
    child: Child,
    pub local_port: u16,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Tunnel {
    /// Returns true if the background ssh process is still running
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Forwards a free local port to `remote_host:remote_port` as seen from the SSH server.
    /// Retries up to 3 times to prevent transient port allocation race conditions (TOCTOU).
    pub fn open(cfg: &SshConfig, remote_host: &str, remote_port: u16) -> Result<Tunnel, String> {
        let mut last_err = String::new();
        for _ in 0..3 {
            match Self::try_open(cfg, remote_host, remote_port) {
                Ok(tunnel) => return Ok(tunnel),
                Err(e) => {
                    last_err = e;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        Err(last_err)
    }

    /// Tests whether an SSH tunnel can be established to the remote target.
    pub fn test_connection(cfg: &SshConfig, remote_host: &str, remote_port: u16) -> Result<(), String> {
        let _tunnel = Self::open(cfg, remote_host, remote_port)?;
        Ok(())
    }

    fn try_open(cfg: &SshConfig, remote_host: &str, remote_port: u16) -> Result<Tunnel, String> {
        let local_port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| e.to_string())?
            .port();
        let mut cmd = quarry_core::command("ssh");
        cmd.args([
            "-N",
            "-L",
            &format!("127.0.0.1:{local_port}:{remote_host}:{remote_port}"),
        ])
        .args(["-p", &cfg.port.to_string()]);

        let auth_mode = cfg.effective_auth_mode();
        let has_secret = cfg
            .secret
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);

        if has_secret {
            if let Ok(exe) = std::env::current_exe() {
                cmd.env("SSH_ASKPASS", exe);
                cmd.env("SSH_ASKPASS_REQUIRE", "force");
                cmd.env("DISPLAY", "dummy:0");
                cmd.env("QUARRY_SSH_ASKPASS", "1");
                if auth_mode == SshAuthMode::Password {
                    cmd.env("QUARRY_SSH_PASSWORD", cfg.secret.as_deref().unwrap_or(""));
                } else {
                    cmd.env("QUARRY_SSH_PASSPHRASE", cfg.secret.as_deref().unwrap_or(""));
                }
            }
            cmd.args([
                "-o",
                "NumberOfPasswordPrompts=1",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "TCPKeepAlive=yes",
            ]);
        } else {
            cmd.args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "TCPKeepAlive=yes",
            ]);
        }

        cmd.args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ConnectTimeout=10",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

        if let Some(jump) = cfg.jump_host.as_deref().filter(|j| !j.trim().is_empty()) {
            cmd.args(["-J", jump.trim()]);
        }

        match auth_mode {
            SshAuthMode::Key => {
                if let Some(key) = cfg.key_path.as_deref().filter(|k| !k.trim().is_empty()) {
                    cmd.args(["-i", &expand_home(key), "-o", "IdentitiesOnly=yes"]);
                }
            }
            SshAuthMode::Password => {
                cmd.args([
                    "-o",
                    "PubkeyAuthentication=no",
                    "-o",
                    "PasswordAuthentication=yes",
                ]);
            }
            SshAuthMode::Agent => {}
        }

        cmd.arg(format!("{}@{}", cfg.user, cfg.host));
        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "ssh isn't installed or isn't on PATH".to_string(),
            _ => e.to_string(),
        })?;

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(Some(_)) = child.try_wait() {
                let mut err = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
                }
                let err_trimmed = err.trim();
                let clean_err = if err_trimmed.is_empty() {
                    "Connection closed by server".to_string()
                } else {
                    clean_ssh_error(err_trimmed)
                };
                return Err(format!("SSH tunnel failed: {clean_err}"));
            }
            if TcpStream::connect(("127.0.0.1", local_port)).is_ok() {
                watch(std::process::id(), child.id());
                return Ok(Tunnel { child, local_port });
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                return Err(format!("SSH tunnel to {} timed out", cfg.host));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Drop only runs on a clean exit. This watchdog stops ssh if Quarry is killed or crashes,
/// and exits on its own once the tunnel is closed normally.
#[cfg(unix)]
fn watch(app_pid: u32, ssh_pid: u32) {
    let script = format!("while kill -0 {app_pid} 2>/dev/null && kill -0 {ssh_pid} 2>/dev/null; do sleep 2; done; kill {ssh_pid} 2>/dev/null");
    let _ = quarry_core::command("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(windows)]
fn watch(app_pid: u32, ssh_pid: u32) {
    let script = format!("while ((Get-Process -Id {app_pid} -EA 0) -and (Get-Process -Id {ssh_pid} -EA 0)) {{ Start-Sleep 2 }}; Stop-Process -Id {ssh_pid} -EA 0");
    let _ = quarry_core::command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(not(any(unix, windows)))]
fn watch(_app_pid: u32, _ssh_pid: u32) {}

pub fn current_os_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

pub fn expand_home(path: &str) -> String {
    let stripped = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"));
    match stripped {
        Some(rest) => {
            if let Some(mut home) = std::env::home_dir() {
                home.push(rest.replace('\\', "/"));
                home.to_string_lossy().to_string()
            } else {
                path.to_string()
            }
        }
        None => path.to_string(),
    }
}

/// A configured host parsed from SSH config files or SSH agent
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}

/// Discovers configured servers from `~/.ssh/config` (including `Include` directives)
pub fn discover_ssh_hosts() -> Vec<DiscoveredSshHost> {
    let mut seen = std::collections::HashSet::new();
    let mut hosts = Vec::new();
    let default_config = expand_home("~/.ssh/config");
    let path = std::path::Path::new(&default_config);
    if path.exists() {
        parse_ssh_config_file(path, &mut seen, &mut hosts);
    }
    hosts
}

fn parse_ssh_config_file(
    path: &std::path::Path,
    seen: &mut std::collections::HashSet<std::path::PathBuf>,
    hosts: &mut Vec<DiscoveredSshHost>,
) {
    let canonical = match path.canonicalize() {
        Ok(c) => c,
        Err(_) => path.to_path_buf(),
    };
    if seen.contains(&canonical) {
        return;
    }
    seen.insert(canonical);

    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };

    let mut current_indices: Vec<usize> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let Some(key) = tokens.next() else { continue };
        let key_lower = key.to_ascii_lowercase();

        if key_lower == "host" {
            current_indices.clear();
            for pattern in tokens {
                if !pattern.contains('*') && !pattern.contains('?') && !pattern.starts_with('!') {
                    let unquoted = pattern.trim_matches('"').trim_matches('\'');
                    if !unquoted.is_empty() {
                        let idx = hosts.len();
                        hosts.push(DiscoveredSshHost {
                            alias: unquoted.to_string(),
                            hostname: None,
                            user: None,
                            port: None,
                            identity_file: None,
                            proxy_jump: None,
                        });
                        current_indices.push(idx);
                    }
                }
            }
        } else if key_lower == "include" {
            for inc in tokens {
                let inc_expanded = expand_home(inc.trim_matches('"').trim_matches('\''));
                let inc_path = std::path::Path::new(&inc_expanded);
                let pattern = if inc_path.is_absolute() {
                    inc_expanded
                } else if let Some(parent) = path.parent() {
                    parent.join(inc_path).to_string_lossy().to_string()
                } else {
                    inc_expanded
                };
                if pattern.contains('*') || pattern.contains('?') {
                    if let Some((dir, file_pattern)) = split_glob_parent(&pattern) {
                        if let Ok(entries) = std::fs::read_dir(dir) {
                            for entry in entries.flatten() {
                                let p = entry.path();
                                if let Some(file_name) = p.file_name().and_then(|n| n.to_str()) {
                                    if glob_match(file_pattern, file_name) {
                                        parse_ssh_config_file(&p, seen, hosts);
                                    }
                                }
                            }
                        }
                    }
                } else {
                    let inc_file = std::path::PathBuf::from(pattern);
                    if inc_file.exists() {
                        parse_ssh_config_file(&inc_file, seen, hosts);
                    }
                }
            }
        } else if !current_indices.is_empty() {
            let val = tokens
                .next()
                .map(|v| v.trim_matches('"').trim_matches('\''));
            if let Some(v) = val {
                for &idx in &current_indices {
                    let entry = &mut hosts[idx];
                    match key_lower.as_str() {
                        "hostname" => {
                            if entry.hostname.is_none() {
                                entry.hostname = Some(v.to_string());
                            }
                        }
                        "user" => {
                            if entry.user.is_none() {
                                entry.user = Some(v.to_string());
                            }
                        }
                        "port" => {
                            if entry.port.is_none() {
                                entry.port = v.parse().ok();
                            }
                        }
                        "identityfile" => {
                            if entry.identity_file.is_none() {
                                entry.identity_file = Some(v.to_string());
                            }
                        }
                        "proxyjump" => {
                            if entry.proxy_jump.is_none() {
                                entry.proxy_jump = Some(v.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn split_glob_parent(pattern: &str) -> Option<(std::path::PathBuf, &str)> {
    let p = std::path::Path::new(pattern);
    let parent = p.parent()?;
    let file = p.file_name()?.to_str()?;
    Some((parent.to_path_buf(), file))
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pat_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    let mut dp = vec![vec![false; text_chars.len() + 1]; pat_chars.len() + 1];
    dp[0][0] = true;

    for i in 1..=pat_chars.len() {
        if pat_chars[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }

    for i in 1..=pat_chars.len() {
        for j in 1..=text_chars.len() {
            if pat_chars[i - 1] == '*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if pat_chars[i - 1] == '?' || pat_chars[i - 1] == text_chars[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }

    dp[pat_chars.len()][text_chars.len()]
}

fn clean_ssh_error(err: &str) -> String {
    let lines: Vec<&str> = err
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with("Warning:"))
        .collect();
    if lines.is_empty() {
        err.to_string()
    } else {
        lines.join("; ")
    }
}

/// The state of the local SSH authentication agent
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshAgentStatus {
    Loaded(Vec<String>),
    Empty,
    Unavailable(String),
}

/// Discovers and normalizes SSH agent socket and PATH in the process environment
pub fn init_ssh_env() {
    #[cfg(unix)]
    {
        let current_path = std::env::var("PATH").unwrap_or_default();
        let mut paths: Vec<&str> = current_path.split(':').collect();
        let candidates = [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/opt/homebrew/sbin",
            "/usr/local/sbin",
        ];
        let mut added = false;
        let mut new_path = current_path.clone();
        for candidate in candidates {
            if std::path::Path::new(candidate).exists() && !paths.contains(&candidate) {
                if !new_path.is_empty() {
                    new_path.push(':');
                }
                new_path.push_str(candidate);
                paths.push(candidate);
                added = true;
            }
        }
        if added {
            std::env::set_var("PATH", new_path);
        }
    }

    let sock_valid = std::env::var("SSH_AUTH_SOCK")
        .ok()
        .map(|s| !s.is_empty() && std::path::Path::new(&s).exists())
        .unwrap_or(false);

    if !sock_valid {
        if let Some(sock) = probe_ssh_agent_socket() {
            tracing::info!("Configured SSH agent socket: {sock}");
            std::env::set_var("SSH_AUTH_SOCK", sock);
        }
    }
}

fn probe_ssh_agent_socket() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let p1 = expand_home("~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock");
        if std::path::Path::new(&p1).exists() {
            return Some(p1);
        }
    }

    #[cfg(target_os = "linux")]
    {
        let p1 = expand_home("~/.1password/agent.sock");
        if std::path::Path::new(&p1).exists() {
            return Some(p1);
        }
    }

    let gpg_sock = expand_home("~/.gnupg/S.gpg-agent.ssh");
    if std::path::Path::new(&gpg_sock).exists() {
        return Some(gpg_sock);
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            for sub in &["gcr/ssh", "keyring/ssh", "openssh_agent"] {
                let p = format!("{runtime_dir}/{sub}");
                if std::path::Path::new(&p).exists() {
                    return Some(p);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(entries) = std::fs::read_dir("/private/tmp") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("com.apple.launchd.") {
                    let listeners = entry.path().join("Listeners");
                    if listeners.exists() {
                        return Some(listeners.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    #[cfg(unix)]
    {
        if let Ok(shell) = std::env::var("SHELL") {
            if let Ok(output) = std::process::Command::new(&shell)
                .args(["-l", "-c", "echo -n \"$SSH_AUTH_SOCK\""])
                .output()
            {
                if output.status.success() {
                    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !s.is_empty() && std::path::Path::new(&s).exists() {
                        return Some(s);
                    }
                }
            }
        }
    }

    None
}

/// Checks the state of ssh-agent and any loaded keys
pub fn check_ssh_agent_status() -> SshAgentStatus {
    let output = match quarry_core::command("ssh-add")
        .arg("-l")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(out) => out,
        Err(e) => return SshAgentStatus::Unavailable(e.to_string()),
    };

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        let mut keys = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 3 {
                    let comment = parts[2..].join(" ");
                    keys.push(comment);
                } else {
                    keys.push(trimmed.to_string());
                }
            }
        }
        if keys.is_empty() {
            SshAgentStatus::Empty
        } else {
            SshAgentStatus::Loaded(keys)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stdout} {stderr}").to_lowercase();
        if combined.contains("no identities")
            || (output.status.code() == Some(1) && !combined.contains("could not open"))
        {
            SshAgentStatus::Empty
        } else {
            let err = stderr.trim();
            SshAgentStatus::Unavailable(if err.is_empty() {
                "Agent unavailable".to_string()
            } else {
                err.to_string()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssh_config_backwards_compatible() {
        // Old config format with key_path
        let json_with_key = r#"{"host":"bastion.example.com","port":22,"user":"root","key_path":"~/.ssh/id_rsa"}"#;
        let cfg: SshConfig = serde_json::from_str(json_with_key).unwrap();
        assert_eq!(cfg.effective_auth_mode(), SshAuthMode::Key);
        assert_eq!(cfg.port, 22);
        assert_eq!(cfg.user, "root");
        assert_eq!(cfg.jump_host, None);

        // Old config format without key_path
        let json_agent = r#"{"host":"bastion.example.com","port":2222,"user":"ubuntu"}"#;
        let cfg: SshConfig = serde_json::from_str(json_agent).unwrap();
        assert_eq!(cfg.effective_auth_mode(), SshAuthMode::Agent);
        assert_eq!(cfg.port, 2222);

        // New config format with explicit password auth and jump_host
        let json_password = r#"{"host":"bastion.example.com","port":22,"user":"admin","auth_mode":"Password","jump_host":"jump.corp.com:2222"}"#;
        let mut cfg: SshConfig = serde_json::from_str(json_password).unwrap();
        assert_eq!(cfg.effective_auth_mode(), SshAuthMode::Password);
        assert_eq!(cfg.jump_host.as_deref(), Some("jump.corp.com:2222"));

        // Secret should not be serialized
        cfg.secret = Some("super_secret_password".to_string());
        let serialized = serde_json::to_string(&cfg).unwrap();
        assert!(!serialized.contains("super_secret_password"));
    }

    #[test]
    fn test_clean_ssh_error() {
        let err = "Warning: Permanently added 'host' to the list of known hosts.\nPermission denied (publickey,password).";
        let cleaned = clean_ssh_error(err);
        assert_eq!(cleaned, "Permission denied (publickey,password).");

        let err2 = "ssh: connect to host example.com port 22: Connection refused";
        let cleaned2 = clean_ssh_error(err2);
        assert_eq!(cleaned2, "ssh: connect to host example.com port 22: Connection refused");
    }

    #[test]
    fn test_init_ssh_env() {
        init_ssh_env();
        // Should not panic and PATH should remain valid
        assert!(std::env::var("PATH").is_ok());
    }
}
