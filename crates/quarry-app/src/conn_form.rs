// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! The Add/Edit Connection form and connecting to saved profiles

use super::*;

impl QuarryWindow {
    pub fn refresh_ssh_discovery(&mut self) {
        self.state.ssh_discovered_hosts = crate::ssh::discover_ssh_hosts();
        let status = crate::ssh::check_ssh_agent_status();
        self.state.ssh_agent_keys = match &status {
            crate::ssh::SshAgentStatus::Loaded(keys) => keys.clone(),
            _ => Vec::new(),
        };
        self.state.ssh_agent_status = status;
    }

    /// The Add Connection modal, filled in from a saved connection; saving keeps its id (and keychain password)
    pub fn open_edit_conn(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_ssh_discovery();
        let Some(p) = self
            .state
            .saved_connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
        else {
            return;
        };
        self.state.editing_conn = Some(p.id.clone());
        self.state.add_conn_tab = 0;
        let driver = p.driver.clone();
        self.state.add_conn_env = p.env;
        self.state.add_conn_read_only = p.read_only;
        self.state.add_conn_ssh = p.ssh.is_some();
        self.state.add_conn_feedback = None;
        let path = p.driver().map_or_else(
            || p.path_or_url.clone(),
            |d| d.take_secret(&p.path_or_url).0,
        );
        if let Some(d) = quarry_core::driver(&driver) {
            self.set_conn_driver(d.id(), window, cx);
        }
        self.set_conn_inputs(Some(&p.name), &path, window, cx);
        let ssh = p.ssh.as_ref();
        if let Some(s) = ssh {
            self.state.add_conn_ssh_auth = s.effective_auth_mode();
        } else {
            self.state.add_conn_ssh_auth = crate::ssh::SshAuthMode::Agent;
        }
        let values = [
            ssh.map(|s| s.host.clone()),
            ssh.map(|s| s.port.to_string()),
            ssh.map(|s| s.user.clone()),
            ssh.and_then(|s| s.key_path.clone()),
            None, // secret left blank: blank keeps saved secret
            ssh.and_then(|s| s.jump_host.clone()),
        ];
        for (input, v) in self.ssh_inputs.clone().iter().zip(values) {
            input.update(cx, |i, cx| i.set_value(v.unwrap_or_default(), window, cx));
        }
        let timeout = p
            .statement_timeout_secs
            .map(|t| t.to_string())
            .unwrap_or_default();
        self.timeout_input
            .update(cx, |i, cx| i.set_value(timeout, window, cx));
        self.state.add_conn_modal_open = true;
        self.conn_name_input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn open_add_conn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_ssh_discovery();
        self.state.editing_conn = None;
        self.state.add_conn_feedback = None;
        self.state.add_conn_tab = 0;
        self.state.add_conn_read_only = false;
        self.state.add_conn_ssh = false;
        self.state.add_conn_ssh_auth = crate::ssh::SshAuthMode::Agent;
        for input in &self.ssh_inputs {
            input.update(cx, |i, cx| i.set_value(String::new(), window, cx));
        }
        // A fresh form starts on the driver's usual local defaults (host, port, user, database),
        // so only what differs needs typing. Never a password: that's always typed
        self.set_conn_driver(self.state.add_conn_driver, window, cx);
        let sample = self.conn_driver().sample();
        self.set_conn_inputs(Some(""), sample, window, cx);
        self.state.add_conn_modal_open = true;
        // The URL / file comes first in the form; the name is optional
        self.conn_path_input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn set_conn_inputs(
        &mut self,
        name: Option<&str>,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(name) = name {
            self.conn_name_input
                .update(cx, |i, cx| i.set_value(name.to_string(), window, cx));
        }
        self.conn_path_input
            .update(cx, |i, cx| i.set_value(path.to_string(), window, cx));
        self.fill_conn_fields(window, cx);
    }

    pub(crate) fn conn_driver(&self) -> &'static dyn quarry_core::Driver {
        quarry_core::driver(self.state.add_conn_driver).unwrap_or(quarry_core::drivers()[0])
    }

    /// Picks the form's driver and builds one input per field it asks for
    pub(crate) fn set_conn_driver(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.add_conn_driver == id && !self.conn_field_inputs.is_empty() {
            return;
        }
        self.state.add_conn_driver = id;
        let fields = self.conn_driver().fields();
        self.conn_field_inputs = fields
            .iter()
            .map(|f| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(f.placeholder)
                        .masked(f.secret)
                })
            })
            .collect();
        // Field edits write the URL, unless the URL is the one writing the fields
        self.conn_field_subs = self
            .conn_field_inputs
            .iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) && !this.conn_syncing {
                        let values: Vec<String> = this
                            .conn_field_inputs
                            .iter()
                            .map(|i| i.read(cx).value().to_string())
                            .collect();
                        let url = this
                            .conn_driver()
                            .join(&values, &this.conn_path_input.read(cx).value());
                        this.conn_syncing = true;
                        this.conn_path_input
                            .update(cx, |i, cx| i.set_value(url, window, cx));
                        this.conn_syncing = false;
                    }
                })
            })
            .collect();
    }

    /// Only replaces an empty field or another driver's sample, never something typed
    pub(crate) fn pick_conn_driver(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.conn_path_input.read(cx).value().trim().to_string();
        self.set_conn_driver(id, window, cx);
        if current.is_empty() || quarry_core::drivers().iter().any(|d| d.sample() == current) {
            self.set_conn_inputs(None, self.conn_driver().sample(), window, cx);
        } else {
            self.fill_conn_fields(window, cx);
        }
        cx.notify();
    }

    /// URL → the driver's fields, unless the fields are the ones writing the URL
    pub(crate) fn fill_conn_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.conn_syncing {
            return;
        }
        let url = self.conn_path_input.read(cx).value().to_string();
        let values = self.conn_driver().split(&url);
        self.conn_syncing = true;
        for (i, v) in self.conn_field_inputs.clone().iter().zip(values) {
            i.update(cx, |i, cx| i.set_value(v, window, cx));
        }
        self.conn_syncing = false;
    }

    /// Connects to a pasted or dropped address, if a driver recognizes it
    pub fn connect_address(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        match ConnectionProfile::from_address(text) {
            Some(profile) => self.connect_profile(profile, window, cx),
            None => self.toast("No database driver recognizes that address", true, cx),
        }
    }

    pub(crate) fn profile_from_modal(&self, cx: &App) -> ConnectionProfile {
        let name = self.conn_name_input.read(cx).value().trim().to_string();
        let path_or_url = self.conn_path_input.read(cx).value().trim().to_string();
        ConnectionProfile {
            id: self
                .state
                .editing_conn
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            name: if name.is_empty() {
                self.conn_driver().display_address(&path_or_url)
            } else {
                name
            },
            driver: self.state.add_conn_driver.to_string(),
            path_or_url,
            env: self.state.add_conn_env,
            read_only: self.state.add_conn_read_only,
            ssh: self.ssh_from_modal(cx),
            ssl: None,
            statement_timeout_secs: self.timeout_input.read(cx).value().trim().parse().ok(),
        }
    }

    pub(crate) fn ssh_from_modal(&self, cx: &App) -> Option<ssh::SshConfig> {
        if !self.state.add_conn_ssh
            || self
                .conn_driver()
                .tunnel_target(self.conn_driver().sample())
                .is_none()
        {
            return None;
        }
        let [host, port, user, key, secret, jump_host] = &self.ssh_inputs;
        let host = host.read(cx).value().trim().to_string();
        let port_str = port.read(cx).value().trim().to_string();
        let (host, port) = if let Ok(p) = port_str.parse::<u16>() {
            (host, p)
        } else {
            match host.rsplit_once(':') {
                Some((h, p)) if p.parse::<u16>().is_ok() => {
                    (h.to_string(), p.parse().unwrap_or(22))
                }
                _ => (host, 22),
            }
        };
        let user = user.read(cx).value().trim().to_string();
        let key = key.read(cx).value().trim().to_string();
        let secret = secret.read(cx).value().trim().to_string();
        let jump_host = jump_host.read(cx).value().trim().to_string();
        let auth_mode = self.state.add_conn_ssh_auth;

        (!host.is_empty()).then(|| ssh::SshConfig {
            host,
            port,
            user: if user.is_empty() {
                ssh::current_os_user()
            } else {
                user
            },
            auth_mode,
            key_path: (!key.is_empty()).then_some(key),
            jump_host: (!jump_host.is_empty()).then_some(jump_host),
            secret: (!secret.is_empty()).then_some(secret),
        })
    }

    /// Connects to a new database using values in the Add Connection modal
    pub fn connect_add_conn_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut profile = self.profile_from_modal(cx);
        // Editing: replace the saved profile in place and reconnect with the new settings
        if let Some(id) = self.state.editing_conn.take() {
            if let Some(ref mut ssh) = profile.ssh {
                if ssh.secret.is_none() {
                    ssh.secret = crate::connections::ssh_secret(&id);
                }
            }
            self.state.disconnect(&id);
            if let Some(slot) = self.state.saved_connections.iter_mut().find(|c| c.id == id) {
                *slot = profile.clone();
            }
            // Open tabs and the active connection hold copies; refresh them so an edit
            // (prod/staging, name, read-only) shows at once, e.g. the prod line along the top
            for tab in &mut self.state.tabs {
                if let Tab::Table {
                    connection: Some(c),
                    ..
                } = tab
                {
                    if c.id == id {
                        *c = profile.clone();
                    }
                }
            }
            if let Some(c) = self.state.active_connection.as_mut().filter(|c| c.id == id) {
                *c = profile.clone();
            }
            if let Err(e) = connections::save(&self.state.saved_connections) {
                self.toast(format!("Could not save connection: {e}"), true, cx);
            }
        }
        self.state.add_conn_feedback = Some(("Connecting…".to_string(), false));
        self.connect_profile(profile, window, cx);
    }

    /// Makes a saved connection the active one, connecting if it isn't open yet
    pub(crate) fn use_connection(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .state
            .active_connection
            .as_ref()
            .is_some_and(|c| c.id == id)
        {
            return;
        }
        if !self.state.activate_open_session(&id) {
            if let Some(profile) = self
                .state
                .saved_connections
                .iter()
                .find(|c| c.id == id)
                .cloned()
            {
                self.connect_profile(profile, window, cx);
            }
        }
        cx.notify();
    }

    /// Sidebar click: switch to a saved connection, reusing its session if it's already open
    pub(crate) fn connect_saved(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Clicking the open connection folds or unfolds its tree
        if self
            .state
            .active_connection
            .as_ref()
            .is_some_and(|c| c.id == id)
        {
            if !self.state.collapsed_connections.remove(&id) {
                self.state.collapsed_connections.insert(id);
            }
            cx.notify();
            return;
        }
        self.state.collapsed_connections.remove(&id);
        if self.state.activate_open_session(&id) {
            cx.notify();
            return;
        }
        if let Some(profile) = self
            .state
            .saved_connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
        {
            self.connect_profile(profile, window, cx);
        }
    }

    pub(crate) fn connect_profile(
        &mut self,
        mut profile: ConnectionProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if profile.statement_timeout_secs.is_none()
            && self.settings.default_statement_timeout_secs > 0
        {
            profile.statement_timeout_secs = Some(self.settings.default_statement_timeout_secs);
        }
        let name = profile.name.clone();
        let id = profile.id.clone();
        if !self.connecting.insert(id.clone()) {
            return;
        }
        self.state.messages.push(format!("Connecting to {}…", name));
        cx.notify();
        let log = self.state.activity_log.clone();
        run_db(
            window,
            cx,
            async move {
                open_session(&profile, log)
                    .await
                    .map(|(s, c, t)| (profile, s, c, t))
            },
            move |this, res, window, cx| match res {
                Ok((profile, session, catalog, tunnel)) => {
                    this.connecting.remove(&id);
                    this.toast(format!("Connected to {}", profile.name), false, cx);
                    session
                        .tx()
                        .set_manual(this.settings.default_transaction_mode == "manual");
                    this.state
                        .apply_connection(profile, session, catalog, tunnel);
                    this.state.add_conn_modal_open = false;
                    this.state.add_conn_feedback = None;
                    this.refocus_root(window, cx);
                }
                Err(e) => {
                    this.connecting.remove(&id);
                    this.toast(format!("Could not connect to {}: {}", name, e), true, cx);
                    this.connecting_for_tab = None;
                    this.state.add_conn_feedback = Some((e, true));
                }
            },
        );
    }

    /// Fetches Oracle's client library (only when asked from the connect form), then connects again
    pub(crate) fn download_oracle_client(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.add_conn_feedback = Some(("Downloading Oracle client…".to_string(), false));
        cx.notify();
        run_db(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(quarry_core::oracle::client::install)
                    .await
                    .map_err(|e| e.to_string())?
            },
            |this, res, window, cx| match res {
                Ok(_) => this.connect_add_conn_modal(window, cx),
                Err(e) => {
                    this.state.add_conn_feedback = Some((e, true));
                    cx.notify();
                }
            },
        );
    }

    /// Tests connection parameters
    pub fn test_add_conn_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut profile = self.profile_from_modal(cx);
        if let Some(ref id) = self.state.editing_conn {
            if let Some(ref mut ssh) = profile.ssh {
                if ssh.secret.is_none() {
                    ssh.secret = crate::connections::ssh_secret(id);
                }
            }
        }
        self.state.add_conn_feedback = Some(("Testing…".to_string(), false));
        cx.notify();

        run_db(
            window,
            cx,
            async move {
                let (session, _, _tunnel) = open_session(&profile, Default::default()).await?;
                session.ping().await.map_err(|e| e.to_string())
            },
            |this, res, _window, _cx| {
                this.state.add_conn_feedback = Some(match res {
                    Ok(dur) => (
                        format!(
                            "Connection successful ({:.2}ms)",
                            dur.as_secs_f64() * 1000.0
                        ),
                        false,
                    ),
                    Err(e) => (e, true),
                });
            },
        );
    }

    /// Opens the system file picker to choose a local SQLite database file
    pub fn browse_sqlite_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose Database File".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let path_str = path.to_string_lossy().to_string();
                this.conn_path_input.update(cx, |i, cx| {
                    i.set_value(path_str.clone(), window, cx);
                });
                let name = this.conn_name_input.read(cx).value().trim().to_string();
                if name.is_empty() {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        this.conn_name_input.update(cx, |i, cx| {
                            i.set_value(stem.to_string(), window, cx);
                        });
                    }
                }
                this.fill_conn_fields(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the system file picker to choose an SSH private key file
    pub fn browse_ssh_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose Private Key".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let path_str = path.to_string_lossy().to_string();
                if let Some(key_input) = this.ssh_inputs.get(3) {
                    key_input.update(cx, |i, cx| {
                        i.set_value(path_str, window, cx);
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Selects a configured SSH host from ~/.ssh/config and populates the SSH fields
    pub fn select_discovered_ssh_host(
        &mut self,
        host_alias: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = self
            .state
            .ssh_discovered_hosts
            .iter()
            .find(|h| h.alias == host_alias)
            .cloned();

        let Some(entry) = entry else { return };

        let host_val = entry.hostname.unwrap_or_else(|| entry.alias.clone());
        let port_val = entry
            .port
            .map(|p| p.to_string())
            .unwrap_or_else(|| "22".to_string());

        if let Some(input) = self.ssh_inputs.first() {
            input.update(cx, |i, cx| i.set_value(host_val, window, cx));
        }

        if let Some(input) = self.ssh_inputs.get(1) {
            input.update(cx, |i, cx| i.set_value(port_val, window, cx));
        }

        if let Some(user) = entry.user {
            if let Some(input) = self.ssh_inputs.get(2) {
                input.update(cx, |i, cx| i.set_value(user, window, cx));
            }
        }

        if let Some(key) = entry.identity_file {
            self.state.add_conn_ssh_auth = crate::ssh::SshAuthMode::Key;
            if let Some(input) = self.ssh_inputs.get(3) {
                input.update(cx, |i, cx| i.set_value(key, window, cx));
            }
        }

        if let Some(jump) = entry.proxy_jump {
            if let Some(input) = self.ssh_inputs.get(5) {
                input.update(cx, |i, cx| i.set_value(jump, window, cx));
            }
        }

        cx.notify();
    }

    /// Tests whether the configured SSH tunnel can connect to the target host and port
    pub fn test_ssh_tunnel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cfg) = self.ssh_from_modal(cx) else {
            self.state.add_conn_feedback = Some(("Please enter an SSH host".to_string(), true));
            cx.notify();
            return;
        };

        let url = self.conn_path_input.read(cx).value().trim().to_string();
        let (target_host, target_port) = self
            .conn_driver()
            .tunnel_target(&url)
            .unwrap_or(("127.0.0.1".to_string(), 22));

        self.state.add_conn_feedback = Some(("Testing SSH tunnel…".to_string(), false));
        cx.notify();

        run_db(
            window,
            cx,
            async move {
                tokio::task::spawn_blocking(move || {
                    crate::ssh::Tunnel::test_connection(&cfg, &target_host, target_port)
                })
                .await
                .map_err(|e| e.to_string())?
            },
            |this, res, _window, _cx| {
                this.state.add_conn_feedback = Some(match res {
                    Ok(()) => ("SSH tunnel connected successfully".to_string(), false),
                    Err(e) => (e, true),
                });
            },
        );
    }
}
