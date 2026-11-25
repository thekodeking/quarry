// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>
// Release builds on Windows are a GUI app: no console window alongside it
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![recursion_limit = "4096"]

//! # Quarry Application (`quarry-app`)
//!
//! Native desktop database client UI built on the GPU-accelerated [GPUI](https://www.gpui.rs) framework.
//!
//! Coordinates multi-connection lifecycle management, asynchronous query execution pipelines on dedicated
//! Tokio background workers, split/tab layouts, schema browser trees, interactive columnar data grids,
//! visual ER foreign-key relationship diagrams, query execution plans, and keyboard-driven command bars.

mod add_conn_modal;
mod blur;
mod bottom_panel;
mod command_bar;
mod commands;
mod completion;
mod conn_form;
mod connections;
mod crash_modal;
mod crash_reporter;
mod db_icons;
mod editor;
mod editor_backend;
mod er_view;
mod execution;
mod export_dialog;
mod files;
mod filter_builder;
mod grid;
mod history;
mod import_wizard;
mod layout;
mod menus;
mod object_menu;
mod params_dialog;
mod peek;
mod pins;
mod plan_view;
mod review_modal;
mod runtime;
mod schema_ops;
mod settings;
mod settings_modal;
mod sidebar;
mod ssh;
mod state;
mod status_bar;
mod tab_switcher;
mod table_detail;
mod tabs;
mod theme;
mod toolbar_menus;
mod ui;
mod updater;
mod usages;
mod vim;
mod window_chrome;

use add_conn_modal::AddConnModal;
use command_bar::CommandBar;
use crash_modal::CrashModal;
use editor::QueryEditor;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{EditorState, Input, InputEvent, InputState};
use gpui::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui::component::table::{TableEvent, TableState};
use gpui::component::{ActiveTheme, Icon, Sizable};
use gpui::*;
use grid::{DataGrid, Fetch, ResultSet, TableSource};
use quarry_core::DatabaseSession;
use review_modal::ReviewModal;
use runtime::{run_db, run_db_bg, stream_db};
use sidebar::Sidebar;
use state::{open_session, AppState, ConnectionProfile, Environment, Tab};
use status_bar::StatusBar;
use std::sync::Arc;
use theme::Theme;
use ui::*;

actions!(
    quarry,
    [
        ToggleCommandBar,
        RunQuery,
        RunAll,
        ReviewChanges,
        ToggleSidebar,
        ToggleBottomPanel,
        NewQuery,
        CloseTab,
        CloseWindow,
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        Dismiss,
        ToggleMaximize,
        OpenSettings,
        ToggleTxMode,
        CommitTx,
        RollbackTx,
        ToggleTheme,
        ExportResults,
        Explain,
        ExplainAnalyze,
        FormatSql,
        ReopenClosedTab,
        TabSwitcherNext,
        TabSwitcherPrev,
        ToggleSplit,
        ToggleSplitDirection,
        FocusNextPane,
        FocusPrevPane,
        SwapSplitPanes,
        PinCurrentTab,
        CommandPalette,
        SwitchSpace1,
        SwitchSpace2,
        SwitchSpace3,
        SwitchSpace4,
        SwitchSpace5,
        SwitchSpace6,
        SwitchSpace7,
        SwitchSpace8,
        SwitchSpace9,
        OpenInSplit,
        SaveFile,
        SaveFileAs,
        OpenFile,
        MenuUp,
        MenuDown,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        SelectNextOccurrence,
        NewConnection,
        ReportIssue,
        ViewLastCrash,
    ]
);

actions!(
    quarry,
    [
        MoveSidebarLeft,
        MoveSidebarRight,
        MoveBottomPanelBottom,
        MoveBottomPanelRight,
        ResetLayout,
        MovePaneLeft,
        MovePaneRight,
        MovePaneUp,
        MovePaneDown,
    ]
);

#[derive(Clone, Copy, PartialEq)]
pub enum RunMode {
    Current,
    All,
    Explain,
    ExplainAnalyze,
}

struct QuarryWindow {
    state: AppState,
    theme: Theme,
    focus_handle: FocusHandle,
    command_input: Entity<InputState>,
    conn_name_input: Entity<InputState>,
    conn_path_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    onboard_input: Entity<InputState>,
    /// SSH tunnel fields in the connection form: host, port, user, key file, secret, jump_host
    ssh_inputs: [Entity<InputState>; 6],
    /// One input per field of the picked driver (`Driver::fields`); kept in sync with the URL field
    conn_field_inputs: Vec<Entity<InputState>>,
    conn_field_subs: Vec<Subscription>,
    /// Set while one side of the URL/fields sync writes the other, so it doesn't echo back
    conn_syncing: bool,
    /// Statement timeout (seconds) in the connection form
    timeout_input: Entity<InputState>,
    /// `Some(dark)` once the user picks a theme; otherwise follow the OS
    /// The picked theme; None follows the OS between Quarry Dark and Light
    theme_choice: Option<Theme>,
    toast_task: Option<Task<()>>,
    session_save_task: Option<Task<()>>,
    editor_intel_task: Option<Task<()>>,
    /// Recently closed tabs, newest last, for ⇧⌘T
    closed_tabs: Vec<(Tab, Vec<(String, Subscription)>)>,
    /// The split view's other (inactive) tab, by id
    split_id: Option<String>,
    /// The active tab is in the right pane; panes stay put when focus moves between them
    split_active_right: bool,
    peek: Option<peek::Peek>,
    settings: settings::Settings,
    /// Settings last pushed into editors and grids (they need a window, so this happens at render)
    applied_settings: Option<settings::Settings>,
    /// Window background last set for the theme (blurred or opaque)
    applied_translucent: Option<bool>,
    /// Windows/Linux: the app menus, drawn in the title bar (macOS has the global menu bar)
    menu_bar: Option<Entity<gpui::component::menu::AppMenuBar>>,
    settings_open: bool,
    /// Theme names for the picker, read when Settings opens
    settings_themes: Vec<String>,
    /// Data-changing statements waiting for a "yes" on a PROD connection
    pending_prod_run: Option<Vec<String>>,
    peek_task: Option<Task<()>>,
    /// File the last toast can reveal in the file manager
    toast_reveal: Option<std::path::PathBuf>,
    export: export_dialog::ExportState,
    import: import_wizard::ImportState,
    params: Option<params_dialog::ParamsDialog>,
    filter_builder: Option<filter_builder::FilterBuilderDialog>,
    /// Open grid context menu: (position, menu, dismiss subscription)
    grid_menu: Option<(
        Point<Pixels>,
        Entity<gpui::component::menu::PopupMenu>,
        Subscription,
    )>,
    /// The query tab whose connection was last synced with the active one (see `sync_tab_connection`)
    synced_tab: Option<String>,
    /// Tab ids, most recently active first (⌃Tab order)
    tab_mru: Vec<String>,
    /// Table diagram: foreign-key hops shown (0 = all connected), and the column under the mouse
    diagram_depth: usize,
    diagram_hover: Option<(String, String, String)>,
    /// Diagram canvas: zoom factor, pan offset, the last mouse position while dragging, and the viewport
    diagram_zoom: f32,
    /// `None` = centred on the table, recomputed as the view resizes; set once you pan or zoom
    diagram_pan: Option<Point<Pixels>>,
    /// Where the centred view put it last frame, so a pan or zoom continues from there
    diagram_auto_pan: std::cell::Cell<Point<Pixels>>,
    diagram_drag: Option<Point<Pixels>>,
    pub diagram_card_offsets: std::collections::HashMap<(String, String), Point<f32>>,
    pub diagram_card_drag: Option<crate::er_view::DiagramCardDrag>,
    diagram_view: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub diagram_layout: std::cell::RefCell<Option<crate::er_view::DiagramLayoutCache>>,
    /// A query tab's connection being reopened; the tab keeps it instead of following the active one meanwhile
    connecting_for_tab: Option<String>,
    /// Profiles mid-connect: a double-click mustn't open two sessions
    connecting: std::collections::HashSet<String>,
    /// The table tab a single sidebar click opened; the next single click replaces it
    preview_tab: Option<String>,
    /// Vim: the first key of a pair (gg, dd, dw…), and Visual mode's fixed end and moving end
    vim_pending: Option<char>,
    /// Which Settings section is showing
    settings_section: usize,
    /// ⌘ scroll travel not yet turned into an editor size step
    zoom_scroll: f32,
    vim_anchor: usize,
    vim_head: usize,
    /// Table tabs from last session, waiting for their connection (connection id, table)
    pending_table_tabs: Vec<(String, files::SavedTable)>,
    /// Results fill the query tab, editor hidden
    results_expanded: bool,
    /// ⌃Tab switcher open, with this index into `tab_mru` highlighted
    tab_switcher: Option<usize>,
    /// Last value entered for each parameter name
    param_memory: std::collections::HashMap<String, String>,
    /// Per-tab subscriptions (grid events), dropped with the tab
    tab_subscriptions: std::collections::HashMap<String, Subscription>,
    panel: bottom_panel::PanelState,
    confirm_close_open: bool,
    force_close: bool,
    pub config_watcher: settings::ConfigWatcher,
    config_watch_task: Option<Task<()>>,
    pub dock: layout::Dock,
    pub active_drag_pane: Option<layout::Pane>,
    pub active_drag_tab: Option<usize>,
    pub active_drag_split_pane: Option<usize>,
    pub pending_crash_report: Option<crash_reporter::CrashReport>,
    /// A newer release found at launch, and whether it's being installed
    available_update: Option<Arc<cargo_packager_updater::Update>>,
    installing_update: bool,
}

impl QuarryWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::new();

        let command_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search tables, actions, or type SQL…")
        });
        // Empty by default: a blank name is filled from the address (host:port/db), never a stale "Local SQLite"
        let conn_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Optional"));
        let conn_path_input = cx.new(|cx| InputState::new(window, cx).default_value(":memory:"));
        let onboard_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Paste a connection URL or a SQLite file path")
        });
        onboard_input.update(cx, |i, cx| i.focus(window, cx));
        let timeout_input = cx.new(|cx| InputState::new(window, cx).placeholder("No timeout"));
        let ssh_inputs = [
            cx.new(|cx| {
                InputState::new(window, cx).placeholder("bastion.example.com")
            }),
            cx.new(|cx| InputState::new(window, cx).placeholder("22")),
            cx.new(|cx| InputState::new(window, cx).placeholder("SSH user")),
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Private key file (e.g. ~/.ssh/id_ed25519)")
            }),
            cx.new(|cx| {
                InputState::new(window, cx).placeholder("SSH password or key passphrase")
            }),
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Jump host (optional, e.g. jump.example.com)")
            }),
        ];

        let subscriptions = vec![
            cx.subscribe_in(
                &command_input,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => {
                        this.state.command_bar_selected_index = 0;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.confirm_command_bar(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &conn_path_input,
                window,
                |this, input, ev: &InputEvent, window, cx| {
                    // Pasting a URL picks the database type (C1)
                    if let InputEvent::Change = ev {
                        let url = input.read(cx).value().to_string();
                        if let Some(d) = quarry_core::detect(&url) {
                            this.set_conn_driver(d.id(), window, cx);
                        }
                        this.fill_conn_fields(window, cx);
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &onboard_input,
                window,
                |this, input, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        let text = input.read(cx).value().trim().to_string();
                        if !text.is_empty() {
                            this.connect_address(&text, window, cx);
                        }
                    }
                },
            ),
        ];
        let theme = Theme::for_mode(os_is_dark(window));
        let export = export_dialog::ExportState::new(window, cx);
        let import = import_wizard::ImportState::new(&theme, window, cx);
        let panel = bottom_panel::PanelState::new(&theme, window, cx);
        let mut this = Self {
            state,
            theme,
            panel,
            focus_handle: cx.focus_handle(),
            command_input,
            conn_name_input,
            conn_path_input,
            _subscriptions: subscriptions,
            tab_subscriptions: Default::default(),
            onboard_input,
            ssh_inputs,
            conn_field_inputs: Vec::new(),
            conn_field_subs: Vec::new(),
            conn_syncing: false,
            timeout_input,
            theme_choice: None,
            toast_task: None,
            session_save_task: None,
            editor_intel_task: None,
            closed_tabs: Vec::new(),
            split_id: None,
            split_active_right: false,
            peek: None,
            settings: settings::Settings::default(),
            applied_settings: None,
            applied_translucent: None,
            menu_bar: (!cfg!(target_os = "macos"))
                .then(|| gpui::component::menu::AppMenuBar::new(cx)),
            settings_open: false,
            settings_themes: Vec::new(),
            pending_prod_run: None,
            peek_task: None,
            toast_reveal: None,
            export,
            import,
            params: None,
            filter_builder: None,
            grid_menu: None,
            synced_tab: None,
            tab_mru: Vec::new(),
            diagram_depth: 1,
            diagram_hover: None,
            diagram_zoom: 1.0,
            diagram_pan: None,
            diagram_auto_pan: Default::default(),
            diagram_drag: None,
            diagram_card_offsets: Default::default(),
            diagram_card_drag: None,
            diagram_view: Default::default(),
            diagram_layout: Default::default(),
            connecting_for_tab: None,
            connecting: Default::default(),
            preview_tab: None,
            vim_pending: None,
            settings_section: 0,
            zoom_scroll: 0.0,
            vim_anchor: 0,
            vim_head: 0,
            pending_table_tabs: Vec::new(),
            results_expanded: false,
            tab_switcher: None,
            param_memory: Default::default(),
            confirm_close_open: false,
            force_close: false,
            config_watcher: settings::ConfigWatcher::new(),
            config_watch_task: None,
            dock: layout::dock(),
            active_drag_pane: None,
            active_drag_tab: None,
            active_drag_split_pane: None,
            pending_crash_report: crash_reporter::check_last_crash(),
            available_update: None,
            installing_update: false,
        };
        this.reload_settings(cx);
        let keymap_errors = settings::apply_keymap(cx);
        if let Some(e) = keymap_errors.first() {
            this.toast(e.clone(), true, cx);
        }
        this.apply_theme(cx);
        this.restore_session(window, cx);
        this._subscriptions.push(cx.on_app_quit(|this, cx| {
            this.save_session(cx);
            async {}
        }));
        cx.spawn(async move |this, cx| {
            let update = cx
                .background_executor()
                .spawn(async { updater::check() })
                .await;
            if let Some(update) = update {
                this.update(cx, |this, cx| {
                    this.available_update = Some(Arc::new(update));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        this.config_watch_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1000))
                .await;
            let res = this.update(cx, |this, cx| {
                let changes = this
                    .config_watcher
                    .poll_changes(std::time::Duration::from_millis(500));
                if changes.settings_changed {
                    this.reload_settings(cx);
                }
                if changes.keymap_changed {
                    let _ = crate::settings::apply_keymap(cx);
                }
            });
            if res.is_err() {
                break;
            }
        }));
        this
    }

    /// Theme follows the OS appearance unless the user picked one
    /// Reads settings.json and applies theme, fonts and defaults (P3, P4)
    fn reload_settings(&mut self, cx: &mut Context<Self>) {
        let (s, mut error) = match settings::try_load() {
            Ok(s) => (s, None),
            Err(e) => (self.settings.clone(), Some(e)),
        };
        self.theme_choice = match s.theme.trim().to_lowercase().as_str() {
            "" | "system" => None,
            name => match Theme::named(name, true) {
                Ok(t) => Some(t),
                Err(e) => {
                    error = Some(e);
                    None
                }
            },
        };
        // Re-paint even when only a user theme's colours changed (same name)
        self.theme.name.clear();
        {
            let kit = gpui::component::theme::Theme::global_mut(cx);
            kit.font_size = px(s.font_size);
            kit.mono_font_size = px(s.mono_size());
        }
        theme::apply_fonts(cx, &s.ui_font_family, &s.mono_font_family);
        self.settings = s;
        if let Some(e) = error {
            self.toast(e, true, cx);
        }
        cx.refresh_windows();
    }

    pub fn dismiss_crash_report(&mut self, cx: &mut Context<Self>) {
        self.pending_crash_report = None;
        crash_reporter::acknowledge_last_crash();
        cx.notify();
    }

    pub fn clear_saved_session(&mut self, cx: &mut Context<Self>) {
        self.close_all_tabs(cx);
        self.closed_tabs.clear();
        self.tab_mru.clear();
        if let Some(path) = files::session_path() {
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        }
        self.dismiss_crash_report(cx);
        self.toast("Saved session cleared (Safe Mode)".to_string(), false, cx);
    }

    pub fn view_last_crash(&mut self, cx: &mut Context<Self>) {
        if let Some(report) = crash_reporter::load_latest_crash() {
            self.pending_crash_report = Some(report);
            cx.notify();
        } else {
            self.toast("No crash reports found".to_string(), false, cx);
        }
    }

    /// Pushes editor and grid settings into every open view
    fn apply_settings_to_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.settings.clone();
        for tab in &self.state.tabs {
            if let Tab::Query { editor, .. } = tab {
                editor.update(cx, |e, cx| {
                    e.set_soft_wrap(s.soft_wrap, window, cx);
                });
            }
        }
        let grids: Vec<_> = self
            .state
            .tabs
            .iter()
            .map(|t| t.grid().clone())
            .chain(self.panel.grids())
            .collect();
        for grid in grids {
            grid.update(cx, |t, cx| {
                t.delegate_mut()
                    .set_prefs(&s.null_display, s.max_column_width);
                cx.notify();
            });
        }
        window.set_rem_size(px(s.font_size));
        self.applied_settings = Some(s);
    }

    fn open_config_file(&mut self, keymap: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (path, starter) = if keymap {
            (
                settings::keymap_path(),
                settings::STARTER_KEYMAP.to_string(),
            )
        } else {
            (settings::settings_path(), settings::starter_settings())
        };
        let Some(path) = path else { return };
        settings::ensure_file(&path, &starter);
        let text = std::fs::read_to_string(&path).unwrap_or(starter);
        self.open_query_tab_with(&text, Some(path), window, cx);
        cx.notify();
    }

    fn sync_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        let mut want = self
            .theme_choice
            .clone()
            .unwrap_or_else(|| Theme::for_mode(os_is_dark(window)));
        if self.settings.window_opacity < 1.0 {
            want = want.with_translucency(self.settings.window_opacity);
        }
        if want.name != self.theme.name || want.bg_window != self.theme.bg_window {
            self.theme = want;
            self.apply_theme(cx);
        }
    }

    fn apply_theme(&mut self, cx: &mut Context<Self>) {
        use gpui::component::theme::{Theme as KitTheme, ThemeMode};
        KitTheme::change(
            if self.theme.is_dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            None,
            cx,
        );
        theme::apply_fonts(
            cx,
            &self.settings.ui_font_family,
            &self.settings.mono_font_family,
        );
        self.theme.apply_to_kit(cx);
        let grids: Vec<_> = self
            .state
            .tabs
            .iter()
            .map(|t| t.grid().clone())
            .chain(self.panel.grids())
            .collect();
        for grid in grids {
            grid.update(cx, |t, cx| {
                t.delegate_mut().set_theme(&self.theme);
                cx.notify();
            });
        }
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_choice = Some(Theme::for_mode(!self.theme.is_dark));
        self.sync_theme(window, cx);
        cx.notify();
    }

    fn increase_font_size(&mut self, _: &IncreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        if self.settings.font_size < settings::Settings::MAX_FONT_SIZE {
            self.change_setting(|s| s.increase_font_size(), cx);
        }
    }

    fn decrease_font_size(&mut self, _: &DecreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        if self.settings.font_size > settings::Settings::MIN_FONT_SIZE {
            self.change_setting(|s| s.decrease_font_size(), cx);
        }
    }

    fn reset_font_size(&mut self, _: &ResetFontSize, _: &mut Window, cx: &mut Context<Self>) {
        if (self.settings.font_size - settings::Settings::default().font_size).abs() > f32::EPSILON
            || self.settings.mono_font_size > 0.0
        {
            self.change_setting(|s| s.reset_font_size(), cx);
        }
    }

    /// One tab's content: editor over results for queries, filter bar over the grid for tables
    fn render_tab_view(&mut self, tab: &Tab, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let grid = div().size_full().child(DataGrid::render(
            tab.grid(),
            &self.theme,
            cx,
            |this, window, cx| this.refresh_active_tab(window, cx),
            |this, window, cx| this.review_changes(&ReviewChanges, window, cx),
            |this, window, cx| this.commit_pending_changes(window, cx),
            |this, _, cx| this.open_export(cx),
            |this, target, window, cx| this.follow_foreign_key(target, window, cx),
            |this, clause, window, cx| this.apply_filter_clause(&clause, window, cx),
            |this, col, window, cx| this.open_filter_builder(col, window, cx),
        ));
        match tab {
            // Config files (settings, keymap) are plain editors: nothing to run
            Tab::Query {
                editor,
                file: Some(f),
                ..
            } if f.extension().is_some_and(|e| e == "json") => div()
                .size_full()
                .child(
                    div().size_full().child(editor.clone()),
                )
                .into_any_element(),
            // Editor over results, with a draggable divider. Until the first run the editor gets the whole tab.
            Tab::Query {
                editor,
                is_running,
                error_message,
                grid: results,
                schema,
                row_limit,
                plan,
                vim_mode,
                ..
            } => {
                let conn = self
                    .state
                    .active_connection
                    .as_ref()
                    .map(|c| (c.name.as_str(), c.env));
                let switches_schema = self.state.dialect().switches_schema();
                let schema_label = switches_schema
                    .then(|| schema.clone().unwrap_or_else(|| "default schema".into()));
                let block = self.settings.vim_mode
                    && self.settings.vim_block_cursor
                    && *vim_mode != state::VimMode::Insert;
                let editor = QueryEditor::render(
                    editor,
                    *is_running,
                    self.settings.soft_wrap,
                    conn,
                    schema_label,
                    *row_limit,
                    self.state.dialect().split_rules(),
                    self.settings.vim_mode.then_some(*vim_mode),
                    block.then(|| self.settings.mono_size()),
                    &self.theme,
                    cx,
                );
                let summary = {
                    let rs = results.read(cx).delegate();
                    rs.results.get(rs.active).map(|r| {
                        let ms = r.execution_time.as_millis();
                        match (r.columns.is_empty(), r.rows.len()) {
                            (true, _) => format!("{} rows affected · {ms} ms", r.rows_affected),
                            (false, n) if r.truncated() => format!(
                                "first {n} of {} rows · add a LIMIT, or Export for all · {ms} ms",
                                r.rows_affected
                            ),
                            (false, 1) => format!("1 row · {ms} ms"),
                            (false, n) => format!("{n} rows · {ms} ms"),
                        }
                    })
                };
                if summary.is_none() && error_message.is_none() && !*is_running {
                    return div().size_full().child(editor).into_any_element();
                }
                let header = h_flex()
                    .h(px(crate::ui::ROW_H))
                    .px_3()
                    .gap_2()
                    .items_center()
                    .bg(t.bg_sidebar)
                    .border_y_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text_secondary)
                            .child("Results"),
                    )
                    .map(|h| match (error_message, *is_running) {
                        (_, true) => {
                            h.child(div().text_sm().text_color(t.text_muted).child("Running…"))
                        }
                        (Some(err), _) => h.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_color(t.danger)
                                .truncate()
                                .child(err.clone()),
                        ),
                        (None, _) => {
                            let text = plan
                                .as_ref()
                                .map(|p| plan_view::summary(p))
                                .or_else(|| summary.clone())
                                .unwrap_or_default();
                            h.child(div().text_sm().text_color(t.text_muted).child(text))
                        }
                    })
                    .child(div().flex_1())
                    .child(
                        zbutton("results-expand")
                            .h(px(crate::ui::SMALL_H))
                            .px_2()
                            .icon(if self.results_expanded {
                                IconName::PanelTopOpen
                            } else {
                                IconName::PanelTopClose
                            })
                            .label(if self.results_expanded {
                                "Show editor"
                            } else {
                                "Hide editor"
                            })
                            .ghost()
                            .small()
                            .tooltip(if self.results_expanded {
                                "Bring the editor back above the results"
                            } else {
                                "Give the results the whole pane"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.results_expanded = !this.results_expanded;
                                cx.notify();
                            })),
                    )
                    .child(
                        zbutton("results-export")
                            .h(px(crate::ui::SMALL_H))
                            .px_2()
                            .icon(IconName::Download)
                            .label("Export")
                            .ghost()
                            .small()
                            .tooltip(ui::format_keys("Export (⇧⌘E)"))
                            .on_click(cx.listener(|this, _, _, cx| this.open_export(cx))),
                    );
                let body = match plan
                    .as_ref()
                    .filter(|_| error_message.is_none() && !*is_running)
                {
                    Some(p) => plan_view::render(p, &t),
                    None => grid.into_any_element(),
                };
                let results = v_flex()
                    .size_full()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(body));
                if self.results_expanded {
                    return results.into_any_element();
                }
                v_resizable("query-split")
                    .on_resize(layout::remember("query-split"))
                    .child(
                        resizable_panel()
                            .size(layout::size("query-split", 0, 260.0))
                            .size_range(px(80.0)..px(1200.0))
                            .child(editor),
                    )
                    .child(resizable_panel().child(results))
                    .into_any_element()
            }
            Tab::Table { view, .. } if *view != table_detail::TableView::Data => v_flex()
                .size_full()
                .child(
                    h_flex()
                        .h(px(CONTROL_H))
                        .w_full()
                        .px_3()
                        .border_b_1()
                        .border_color(t.border)
                        .items_center()
                        .child(self.render_table_view_switch(*view, cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_table_detail(tab, cx)),
                )
                .into_any_element(),
            Tab::Table { filter, view, .. } => {
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .h(px(CONTROL_H))
                            .w_full()
                            .px_3()
                            .gap_2()
                            .border_b_1()
                            .border_color(t.border)
                            .items_center()
                            .child(self.render_table_view_switch(*view, cx))
                            .child(div().w_px().h(px(16.0)).bg(t.border))
                            // Single-line inputs propagate Enter: apply the filter
                            .on_action(cx.listener(
                                |this, _: &gpui::component::input::Enter, window, cx| {
                                    this.refresh_active_tab(window, cx);
                                },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .child(ui::search_input(filter, IconName::ListFilter)),
                            )
                            .child(
                                zbutton("open-filter-builder-btn")
                                    .pad_xs()
                                    .icon(IconName::ListFilter)
                                    .label("Filter builder")
                                    .ghost()
                                    .small()
                                    .tooltip(crate::ui::format_keys(
                                        "Visual filter condition builder (⌥F)",
                                    ))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_filter_builder(None, window, cx);
                                    })),
                            )
                            .when(!filter.read(cx).value().is_empty(), |d| {
                                d.child(
                                    zbutton("filter-clear-btn")
                                        .pad_xs()
                                        .label("Clear")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.clear_filter(window, cx)
                                        })),
                                )
                            }),
                    )
                    .child(div().flex_1().min_h_0().child(grid))
                    .into_any_element()
            }
        }
    }

    /// Nothing open: the everyday starting points, Zed-style, plus recent tables
    fn render_launcher(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let row = |id: ElementId, icon: IconName, label: SharedString, hint: &'static str| {
            h_flex()
                .id(id)
                .h(px(crate::ui::CONTROL_H))
                .px_3()
                .gap_3()
                .items_center()
                .rounded(px(crate::ui::RADIUS_MD))
                .cursor_pointer()
                .hover(|s| s.bg(t.bg_hover))
                .child(Icon::new(icon).small().text_color(t.text_muted))
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(t.text_primary)
                        .truncate()
                        .child(label),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(t.text_muted)
                        .child(ui::format_keys(hint)),
                )
        };
        let start = v_flex()
            .gap_0p5()
            .child(
                row(
                    "l-new".into(),
                    IconName::SquareTerminal,
                    "New query".into(),
                    "⌘N",
                )
                .on_click(cx.listener(|this, _, window, cx| this.new_query(&NewQuery, window, cx))),
            )
            .child(
                row(
                    "l-search".into(),
                    IconName::Search,
                    "Search tables & commands".into(),
                    "⌘K",
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_command_bar(&ToggleCommandBar, window, cx)
                })),
            )
            .child(
                row(
                    "l-open".into(),
                    IconName::FolderOpen,
                    "Open file".into(),
                    "⌘O",
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_sql_file(window, cx))),
            )
            .child(
                row(
                    "l-settings".into(),
                    IconName::Settings2,
                    "Settings".into(),
                    "⌘,",
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
            );
        let recent: Vec<(String, String)> =
            self.state.recent_tables.iter().take(5).cloned().collect();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(380.0))
                    .gap_5()
                    .child(
                        v_flex()
                            .px_3()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text_primary)
                                    .child(
                                        self.state.active_connection.as_ref().map_or_else(
                                            || "Quarry".to_string(),
                                            |c| c.name.clone(),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(t.text_muted)
                                    .child("Pick a table from the sidebar, or start here"),
                            ),
                    )
                    .child(start)
                    .when(!recent.is_empty(), |d| {
                        d.child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .px_3()
                                        .pb_1()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.text_muted)
                                        .child("Recent"),
                                )
                                .children(recent.into_iter().enumerate().map(
                                    |(i, (schema, table))| {
                                        row(
                                            ("l-recent", i).into(),
                                            IconName::Table,
                                            table.clone().into(),
                                            "",
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.open_pin(
                                                    pins::Pin::Table {
                                                        schema: schema.clone(),
                                                        table: table.clone(),
                                                    },
                                                    window,
                                                    cx,
                                                )
                                            }),
                                        )
                                    },
                                )),
                        )
                    }),
            )
            .into_any_element()
    }

    /// "Run on PROD?" for statements that change data
    fn render_prod_confirm(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let statements = self.pending_prod_run.as_ref()?;
        let t = &self.theme;
        let name = self
            .state
            .active_connection
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let count = statements
            .iter()
            .filter(|s| quarry_core::session::is_mutation(s))
            .count();
        Some(
            crate::ui::fade_in("prod-backdrop-fade", modal_backdrop("prod-backdrop", dismiss_on_click, cx)
                .child(
                    modal_card("prod-card", 460.0, t)
                        .p_4()
                        .gap_3()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.text_primary).child(format!("Change data on {name} (PROD)?")))
                        .child(div().text_sm().text_color(t.text_secondary).child(format!(
                            "{count} of these statements change data or schema. Turn this check off with \"confirm_on_prod\" in settings."
                        )))
                        .children(statements.iter().take(5).map(|s| {
                            div().text_sm().font_family(cx.theme().mono_font_family.clone()).text_color(t.text_muted).truncate().child(s.clone())
                        }))
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(zbutton("prod-no").pad_sm().label("Cancel").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                                    this.pending_prod_run = None;
                                    cx.notify();
                                })))
                                .child(zbutton("prod-yes").pad_sm().label("Run on PROD").danger().small().on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(statements) = this.pending_prod_run.take() {
                                        this.execute_statements_now(statements, window, cx);
                                    }
                                }))),
                        ),
                ))
                .into_any_element(),
        )
    }

    /// Returns descriptions of any pending/unsaved changes across grids, files, and transactions
    pub fn unsaved_changes(&self, cx: &App) -> Vec<String> {
        let mut changes = Vec::new();

        // 1. Data grid pending changes (cell edits, row inserts, row deletes)
        for tab in &self.state.tabs {
            if let Tab::Table {
                table_name, grid, ..
            } = tab
            {
                let pending = grid.read(cx).delegate().pending.total_count();
                if pending > 0 {
                    changes.push(format!(
                        "{pending} uncommitted {} in '{table_name}'",
                        if pending == 1 { "edit" } else { "edits" }
                    ));
                }
            }
        }
        for grid in self.panel.grids() {
            let pending = grid.read(cx).delegate().pending.total_count();
            if pending > 0 {
                changes.push(format!(
                    "{pending} uncommitted {} in query results",
                    if pending == 1 { "edit" } else { "edits" }
                ));
            }
        }

        // 2. Unsaved query tabs (file-backed or scratch with edits)
        for tab in &self.state.tabs {
            if let Tab::Query {
                title,
                editor,
                file,
                saved_text,
                ..
            } = tab
            {
                let text = editor.read(cx).value();
                if text.as_ref() != saved_text.as_str() {
                    if let Some(f) = file {
                        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or(title);
                        changes.push(format!("Unsaved file '{name}'"));
                    } else if !text.trim().is_empty() {
                        changes.push(format!("Unsaved query in '{title}'"));
                    }
                }
            }
        }

        // 3. Active uncommitted transaction
        if self
            .current_connection(cx)
            .0
            .is_some_and(|s| s.in_transaction())
        {
            changes.push("Open database transaction (will be rolled back)".into());
        }

        // 4. Running queries
        for tab in &self.state.tabs {
            if let Tab::Query {
                title, is_running, ..
            } = tab
            {
                if *is_running {
                    changes.push(format!("Running query in '{title}'"));
                }
            }
        }

        changes
    }

    pub fn has_unsaved_changes(&self, cx: &App) -> bool {
        !self.unsaved_changes(cx).is_empty()
    }

    fn handle_close_window(
        &mut self,
        _: &CloseWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.force_close || !self.has_unsaved_changes(cx) {
            window.remove_window();
        } else {
            self.confirm_close_open = true;
            cx.notify();
        }
    }

    fn handle_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        if self.force_close || !self.has_unsaved_changes(cx) {
            cx.quit();
        } else {
            self.confirm_close_open = true;
            cx.notify();
        }
    }

    /// "Discard unsaved changes?" modal when closing window
    fn render_close_confirm(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.confirm_close_open {
            return None;
        }
        let changes = self.unsaved_changes(cx);
        let t = &self.theme;
        let count = changes.len();
        Some(
            crate::ui::fade_in("close-confirm-backdrop-fade", modal_backdrop("close-confirm-backdrop", dismiss_on_click, cx)
                .child(
                    modal_card("close-confirm-card", 460.0, t)
                        .p_4()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_primary)
                                .child("Discard unsaved changes?"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(t.text_secondary)
                                .child(if count == 1 {
                                    "You have unsaved changes that will be lost if you close the window:"
                                } else {
                                    "You have multiple unsaved changes that will be lost if you close the window:"
                                }),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .children(changes.into_iter().take(6).map(|item| {
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(div().size(px(4.0)).rounded_full().bg(t.text_muted))
                                        .child(div().text_sm().text_color(t.text_primary).truncate().child(item))
                                })),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    zbutton("close-confirm-cancel")
                                        .pad_sm()
                                        .label("Cancel")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_close_open = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    zbutton("close-confirm-discard")
                                        .pad_sm()
                                        .label("Close window")
                                        .danger()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, _cx| {
                                            this.confirm_close_open = false;
                                            this.force_close = true;
                                            window.remove_window();
                                        })),
                                ),
                        ),
                ))
                .into_any_element(),
        )
    }

    /// First run (U11): one input, nothing else
    fn render_onboarding(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(520.0))
                    .gap_3()
                    .child(div().text_lg().text_color(t.text_primary).child("Connect a database"))
                    .child(Input::new(&self.onboard_input))
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_muted)
                            .child("A URL like postgres://user@host/db, a file path, or drop a .sqlite file here"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                zbutton("onboard-form").pad_sm()
                                    .icon(IconName::Settings2)
                                    .label("More options")
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| this.open_add_conn(window, cx))),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(update) = self.available_update.clone() else {
            return;
        };
        if self.installing_update {
            return;
        }
        self.installing_update = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { updater::install(&update) })
                .await;
            this.update(cx, |this, cx| match res {
                Ok(()) => cx.quit(),
                Err(e) => {
                    this.installing_update = false;
                    this.toast(format!("Update failed: {e}"), true, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Short-lived confirmation in the corner; also kept in Output
    fn toast(&mut self, msg: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        let msg = msg.into();
        self.state.messages.push(if error {
            format!("Error: {msg}")
        } else {
            msg.clone()
        });
        self.state.toast_message = Some((msg, error));
        self.toast_reveal = None;
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            // Errors stay long enough to read, e.g. a connection that failed while the window opened
            cx.background_executor()
                .timer(std::time::Duration::from_secs(if error { 10 } else { 4 }))
                .await;
            this.update(cx, |this, cx| {
                this.state.toast_message = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The connection the active tab uses: a table tab's own, otherwise the active one
    fn current_connection(
        &self,
        cx: &App,
    ) -> (Option<Arc<dyn DatabaseSession>>, Option<ConnectionProfile>) {
        match self.state.active_tab() {
            Some(Tab::Table {
                grid, connection, ..
            }) => (
                grid.read(cx)
                    .delegate()
                    .source
                    .as_ref()
                    .map(|s| s.session.clone()),
                connection.clone(),
            ),
            _ => (
                self.state.session.clone(),
                self.state.active_connection.clone(),
            ),
        }
    }

    fn open_query_tab(&mut self, sql: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_query_tab_with(sql, None, window, cx);
    }

    pub fn open_query_tab_with(
        &mut self,
        sql: &str,
        file: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = self
            .state
            .tabs
            .iter()
            .filter(|t| matches!(t, Tab::Query { .. }))
            .count()
            + 1;
        let language = if file
            .as_ref()
            .is_some_and(|f| f.extension().is_some_and(|e| e == "json"))
        {
            "json"
        } else {
            "sql"
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(self.settings.line_numbers)
                .soft_wrap(self.settings.soft_wrap)
                .folding(self.settings.code_folding)
                .smart_indent(true)
                .auto_close(true)
                .tab_size(gpui::base::input::TabSize {
                    tab_size: self.settings.tab_size.max(1),
                    hard_tabs: false,
                })
                .placeholder(ui::format_keys("Write SQL…  ⌘↵ to run"))
                .default_value(sql.to_string())
        });
        let completions = std::rc::Rc::new(completion::SchemaCompletions::new(
            self.state.completion_catalog.clone(),
            Some(self.state.completion_usage.clone()),
            Some(self.state.completion_history.clone()),
        ));
        let me = cx.entity().downgrade();
        editor.update(cx, |e, cx| {
            e.lsp_mut().completion_menu.max_width = gpui::px(480.);
            e.lsp_mut().completion_provider = self.settings.autocomplete.then_some(completions);
            e.lsp_mut().hover_provider = Some(std::rc::Rc::new(completion::SchemaHover(
                self.state.completion_catalog.clone(),
            )));
            e.lsp_mut()
                .code_action_providers
                .push(std::rc::Rc::new(completion::SqlActions(
                    self.state.completion_catalog.clone(),
                    self.state.completion_dialect.clone(),
                )));
            e.lsp_mut().definition_provider = Some(std::rc::Rc::new(completion::TableDefinitions(
                self.state.completion_catalog.clone(),
            )));
            let me = me.clone();
            e.lsp_mut().show_document = Some(std::rc::Rc::new(move |params, window, cx| {
                let Some((schema, table)) = completion::table_link(&params.uri) else {
                    return false;
                };
                let me = me.clone();
                window.defer(cx, move |window, cx| {
                    me.update(cx, |this, cx| {
                        this.open_pin(pins::Pin::Table { schema, table }, window, cx)
                    })
                    .ok();
                });
                true
            }));
            e.focus(window, cx);
        });
        let id = format!("q-{}", uuid::Uuid::new_v4());
        // Every edit schedules a session save, so unsaved queries survive a restart (Q9)
        completion::lint_editor(&editor, &self.state.completion_catalog, cx);
        let sub = cx.subscribe_in(&editor, window, |this, editor, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                if editor.read(cx).value().len() > 512 * 1024 {
                    editor.update(cx, |e, cx| e.set_soft_wrap(false, window, cx));
                }
                this.schedule_editor_intelligence(editor.clone(), cx);
                this.schedule_session_save(cx);
                cx.notify();
            }
        });
        self.tab_subscriptions.insert(format!("{id}:editor"), sub);
        self.tab_subscriptions.insert(
            format!("{id}:statement"),
            self.track_current_statement(&editor, cx),
        );
        let grid = self.new_grid(&id, None, window, cx);
        let title = file
            .as_ref()
            .and_then(|f| f.file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("query {}", n));
        self.state.tabs.push(Tab::Query {
            id,
            title,
            editor,
            grid,
            file,
            saved_text: sql.to_string(),
            connection_id: self.state.active_connection.as_ref().map(|c| c.id.clone()),
            schema: None,
            row_limit: None,
            is_running: false,
            error_message: None,
            plan: None,
            vim_mode: state::VimMode::Normal,
        });
        self.state.active_tab_index = self.state.tabs.len() - 1;
        self.schedule_session_save(cx);
    }

    pub fn open_table_tab(
        &mut self,
        table_name: &str,
        schema_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.note_recent_table(schema_name, table_name);
        if let Some(idx) = self.state.find_tab(table_name) {
            self.state.active_tab_index = idx;
            return;
        }
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter rows, e.g. id > 1"));
        // Re-render as it's typed in, so Clear shows up once there's something to clear
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        let Some(session) = self.state.session.clone() else {
            return;
        };
        let schema = self
            .state
            .catalog
            .tables
            .iter()
            .find(|t| t.schema_name == schema_name && t.table_name == table_name);
        let read_only_reason = match schema {
            _ if session.is_read_only() => Some("Read-only connection"),
            Some(t) if t.is_view => Some("Views are read-only"),
            Some(t) if t.primary_keys.is_empty() => Some("Read-only: table has no primary key"),
            None => Some("Read-only: table not found in schema"),
            _ => None,
        };
        let (columns, primary_keys, foreign_keys) = schema
            .map(|t| {
                (
                    t.columns.clone(),
                    t.primary_keys.clone(),
                    t.foreign_keys.clone(),
                )
            })
            .unwrap_or_default();
        let id = format!("t-{}.{}", schema_name, table_name);
        let grid = self.new_grid(
            &id,
            Some(TableSource {
                session,
                schema: schema_name.to_string(),
                table: table_name.to_string(),
                columns,
                primary_keys,
                read_only_reason,
                foreign_keys,
                page_size: self.settings.rows_per_page,
                filter: String::new(),
                order_by: None,
                offset: 0,
                total: None,
                counting: false,
                has_more: false,
                loading: false,
                error: None,
            }),
            window,
            cx,
        );
        self.state.tabs.push(Tab::Table {
            id,
            table_name: table_name.to_string(),
            schema_name: schema_name.to_string(),
            filter,
            grid,
            connection: self.state.active_connection.clone(),
            view: Default::default(),
            detail: None,
        });
        self.state.active_tab_index = self.state.tabs.len() - 1;
        self.schedule_session_save(cx);
    }

    /// Opens the referenced table filtered to the row a foreign-key cell points at (G5)
    fn follow_foreign_key(
        &mut self,
        (schema, table, filter): (String, String, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_table_tab(&table, &schema, window, cx);
        if let Some(Tab::Table { filter: input, .. }) = self.state.active_tab() {
            input
                .clone()
                .update(cx, |i, cx| i.set_value(filter, window, cx));
        }
        self.refresh_active_tab(window, cx);
        cx.notify();
    }

    /// Tints the statement ⌘↵ would run, and keeps the gutter ▶ next to it
    fn track_current_statement(
        &self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.observe(editor, |_, _, _| {})
    }

    fn open_grid_menu(
        &mut self,
        grid: Entity<TableState<ResultSet>>,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = grid.update(cx, |t, cx| {
            t.set_selected_cell(row, col, cx);
            t.delegate_mut().selected_cell = Some((row, col));
            t.focus_handle(cx)
        });
        let menu = gpui::component::menu::PopupMenu::build(window, cx, |menu, _, cx| {
            grid::grid_menu(menu.action_context(focus), grid.read(cx).delegate())
        });
        menu.update(cx, |m, cx| m.focus_handle(cx).focus(window, cx));
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.grid_menu = None;
            cx.notify();
        });
        self.grid_menu = Some((window.mouse_position(), menu, sub));
        cx.notify();
    }

    fn new_grid(
        &mut self,
        tab_id: &str,
        source: Option<TableSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<ResultSet>> {
        let mut rs = ResultSet::new(&self.theme, source);
        rs.set_prefs(&self.settings.null_display, self.settings.max_column_width);
        let grid = cx.new(|cx| {
            TableState::new(rs, window, cx)
                .cell_selectable(true)
                .col_resizable(true)
                .col_selectable(false)
        });
        // Double-click edits the cell (read-only grids explain why they can't)
        let sub = cx.subscribe_in(&grid, window, |this, grid, ev: &TableEvent, window, cx| {
            match *ev {
                TableEvent::DoubleClickedCell(row, col) => grid.update(cx, |t, cx| {
                    t.delegate_mut().start_edit(row, col, None, window, cx)
                }),
                // Cell-selection mode handles right-clicks itself, so open the menu here
                TableEvent::RightClickedCell(row, col) => {
                    this.open_grid_menu(grid.clone(), row, col, window, cx)
                }
                // The value editor follows the selection
                TableEvent::SelectCell(row, col) => grid.update(cx, |t, cx| {
                    let rs = t.delegate_mut();
                    if rs.is_dragging {
                        // Dragging in progress, keep selected_range intact!
                    } else if let Some(ref range) = rs.selected_range {
                        if !range.is_single_cell() && range.contains(row, col) {
                            // Multi-cell range selected, keep it!
                        } else {
                            rs.selected_cell = Some((row, col));
                            rs.selected_range = Some(grid::CellRange::single(row, col));
                        }
                    } else {
                        rs.selected_cell = Some((row, col));
                        rs.selected_range = Some(grid::CellRange::single(row, col));
                    }
                    if t.delegate().inspector_open() {
                        t.delegate_mut().inspect(row, col, window, cx);
                    }
                }),
                TableEvent::ClearSelection => grid.update(cx, |t, _| {
                    t.delegate_mut().clear_selection_range();
                }),
                _ => {}
            }
        });
        self.tab_subscriptions.insert(tab_id.to_string(), sub);
        grid
    }

    fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Tab::Table { filter, .. }) = self.state.active_tab() {
            filter
                .clone()
                .update(cx, |f, cx| f.set_value("", window, cx));
        }
        self.refresh_active_tab(window, cx);
    }

    pub fn refocus_root(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    // ── Actions ─────────────────────────────────────────────

    fn toggle_command_bar(
        &mut self,
        _: &ToggleCommandBar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.command_bar_open = !self.state.command_bar_open;
        self.state.usages = None; // ⌘K is the normal palette, not the last Find Usages
        if self.state.command_bar_open {
            self.state.command_bar_selected_index = 0;
            // Opens on commands; deleting the ">" searches tables and queries
            self.command_input.update(cx, |input, cx| {
                input.set_value(">", window, cx);
                input.focus(window, cx);
            });
        } else {
            self.refocus_root(window, cx);
        }
        cx.notify();
    }

    fn run_query(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.review_modal_open {
            return self.commit_pending_changes(window, cx);
        }
        self.refresh_active_tab(window, cx);
    }

    fn run_all(&mut self, _: &RunAll, window: &mut Window, cx: &mut Context<Self>) {
        self.run_active_tab(true, window, cx);
    }

    fn review_changes(&mut self, _: &ReviewChanges, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state.active_tab(), Some(Tab::Query { .. })) {
            return self.save_query(false, window, cx);
        }
        let review_data = match self.state.active_tab() {
            Some(Tab::Table {
                table_name, grid, ..
            }) => {
                let rs = grid.read(cx).delegate();
                rs.source
                    .as_ref()
                    .filter(|_| !rs.pending.is_empty())
                    .map(|src| {
                        (
                            table_name.clone(),
                            rs.pending
                                .generate_sql(&src.qualified_name(), src.dialect()),
                            rs.pending.clone(),
                        )
                    })
            }
            _ => None,
        };
        if let Some((table, sqls, changes)) = review_data {
            self.state.review_modal_sql = sqls;
            self.state.review_modal_table = table;
            self.state.review_modal_changes = changes;
            self.state.review_modal_show_sql = false;
            self.state.review_modal_page = 0;
            self.state.review_modal_scroll = Default::default();
            self.state.review_modal_open = true;
            cx.notify();
        }
    }

    fn schedule_editor_intelligence(
        &mut self,
        editor: Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        self.editor_intel_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;

            let Some((sql, completion_catalog, schema_catalog)) = this
                .update(cx, |this, cx| {
                    let sql = editor.read(cx).value().to_string();
                    (
                        sql,
                        this.state.completion_catalog.borrow().clone(),
                        this.state.catalog.clone(),
                    )
                })
                .ok()
            else {
                return;
            };

            if sql.len() > 200_000 {
                return;
            }

            let (_found, mentioned) = cx
                .background_executor()
                .spawn({
                    let sql = sql.clone();
                    async move {
                        let found = quarry_sql::lint(&sql, &completion_catalog);
                        let mentioned = quarry_sql::mentioned_unloaded(&sql, &schema_catalog);
                        (found, mentioned)
                    }
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if !mentioned.is_empty() {
                    this.load_schemas(mentioned, cx);
                }
            });
        }));
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.state.sidebar_visible = !self.state.sidebar_visible;
        cx.notify();
    }

    fn toggle_bottom_panel(
        &mut self,
        _: &ToggleBottomPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.bottom_panel_open = !self.state.bottom_panel_open;
        cx.notify();
    }

    /// Query tabs keep their own connection. Landing on a query tab switches to its connection;
    /// switching connections (sidebar, picker) while on one moves the tab to the new connection.
    fn sync_tab_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.state.active_connection.as_ref().map(|c| c.id.clone());
        let Some(Tab::Query {
            id, connection_id, ..
        }) = self.state.active_tab()
        else {
            self.synced_tab = None;
            return;
        };
        let (id, wanted) = (id.clone(), connection_id.clone());
        if self.synced_tab.as_ref() != Some(&id) {
            self.synced_tab = Some(id);
            if let Some(w) = wanted.filter(|w| current.as_ref() != Some(w)) {
                if self.state.activate_open_session(&w) {
                    return;
                }
                // Not open (e.g. after a restart): reconnect, and leave the tab alone until that lands
                if let Some(profile) = self
                    .state
                    .saved_connections
                    .iter()
                    .find(|c| c.id == w)
                    .cloned()
                {
                    self.connecting_for_tab = Some(w);
                    self.connect_profile(profile, window, cx);
                    return;
                }
            }
        }
        if let Some(pending) = &self.connecting_for_tab {
            if current.as_ref() != Some(pending) {
                return;
            }
            self.connecting_for_tab = None;
        }
        if let Some(Tab::Query { connection_id, .. }) =
            self.state.tabs.get_mut(self.state.active_tab_index)
        {
            *connection_id = current;
        }
    }

    fn select_next_occurrence(
        &mut self,
        _: &SelectNextOccurrence,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Tab::Query { editor, .. }) = self.state.active_tab() else {
            return;
        };
        let editor = editor.clone();
        editor.update(cx, |ed, cx| {
            let val = ed.value();
            let text = val.as_str();
            let range = ed.selected_range();
            if range.start < range.end && range.end <= text.len() {
                if let Some(needle) = text.get(range.start..range.end) {
                    let search_from = range.end;
                    let found = text[search_from..]
                        .find(needle)
                        .map(|offset| search_from + offset)
                        .or_else(|| text[..range.start].find(needle));
                    if let Some(start) = found {
                        let end = start + needle.len();
                        ed.set_selected_range(start..end, cx);
                    }
                }
            } else {
                let cursor = range.start.min(text.len());
                let bytes = text.as_bytes();
                let mut start = cursor;
                while start > 0
                    && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
                {
                    start -= 1;
                }
                let mut end = cursor;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                if start < end {
                    ed.set_selected_range(start..end, cx);
                }
            }
        });
    }

    pub fn handle_vim_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use gpui::component::input as act;
        use state::VimMode;
        let Some(Tab::Query {
            vim_mode, editor, ..
        }) = self.state.active_tab()
        else {
            return false;
        };
        let (mode, editor) = (*vim_mode, editor.clone());
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        let escape = ks.key == "escape" || (m.control && (ks.key == "[" || ks.key == "c"));

        if mode == VimMode::Insert {
            if escape {
                // Leaving insert steps back onto the last typed character, as Vim does
                window.dispatch_action(Box::new(act::MoveLeft), cx);
                self.set_vim_mode(VimMode::Normal, cx);
                return true;
            }
            return false;
        }
        if m.platform || m.alt {
            return false;
        }
        let (text, cursor) = {
            let st = editor.read(cx);
            (st.value().to_string(), st.cursor())
        };
        // Undo / redo restore their text selected; Vim leaves just the cursor at its start.
        // Deferred, because the dispatched action itself only runs after this handler
        let collapse_after =
            |editor: &Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>| {
                let editor = editor.clone();
                window.defer(cx, move |_, cx| {
                    editor.update(cx, |st, cx| {
                        let start = st.selected_range().start;
                        st.set_selected_range(start..start, cx)
                    });
                });
            };
        let select = |range: std::ops::Range<usize>, cx: &mut Context<Self>| {
            editor.update(cx, |st, cx| st.set_selected_range(range, cx));
        };
        if escape {
            self.vim_pending = None;
            if mode == VimMode::Visual {
                select(self.vim_head..self.vim_head, cx);
                self.set_vim_mode(VimMode::Normal, cx);
            }
            return true;
        }
        if m.control {
            match ks.key.as_str() {
                "r" => {
                    window.dispatch_action(Box::new(act::Redo), cx);
                    collapse_after(&editor, window, cx);
                }
                "d" => window.dispatch_action(Box::new(act::MovePageDown), cx),
                "u" => window.dispatch_action(Box::new(act::MovePageUp), cx),
                _ => return false,
            }
            return true;
        }
        // Typed form: shift-4 is "$", shift-a is "A"
        let key = ks.key_char.clone().unwrap_or_else(|| ks.key.clone());
        let key = key.as_str();

        if mode == VimMode::Visual {
            let head = self.vim_head;
            if let Some(to) = vim::motion(&text, head, key) {
                self.vim_head = to;
                let r = vim::op_range(self.vim_anchor, to);
                select(r.start..vim::char_after(&text, r.end), cx);
                return true;
            }
            match key {
                "y" => {
                    window.dispatch_action(Box::new(act::Copy), cx);
                    let start = self.vim_anchor.min(head);
                    select(start..start, cx);
                }
                "d" | "x" => window.dispatch_action(Box::new(act::Cut), cx),
                "c" => {
                    window.dispatch_action(Box::new(act::Cut), cx);
                    self.set_vim_mode(VimMode::Insert, cx);
                    return true;
                }
                "v" => select(head..head, cx),
                _ => return ks.key_char.is_some(),
            }
            self.set_vim_mode(VimMode::Normal, cx);
            return true;
        }

        // Normal mode: a pending g / d / y / c takes this key as its second half
        if let Some(op) = self.vim_pending.take() {
            let range = match (op, key) {
                ('g', "g") => {
                    select(0..0, cx);
                    return true;
                }
                (_, k) if k.len() == 1 && k.starts_with(op) => {
                    let line = vim::line_range(&text, cursor);
                    if op == 'c' {
                        // cc keeps the (now empty) line to type on
                        let t = text[line.clone()].trim_matches('\n');
                        let at = line.start + text[line.clone()].find(t).unwrap_or(0);
                        at..at + t.len()
                    } else {
                        line
                    }
                }
                (_, k) => match vim::motion(&text, cursor, k) {
                    // e lands on the word's last character, which the operator includes
                    Some(to) if k == "e" => cursor..vim::char_after(&text, to),
                    Some(to) => vim::op_range(cursor, to),
                    None => return true,
                },
            };
            self.vim_operate(op, range, &text, &editor, window, cx);
            return true;
        }
        if let Some(to) = vim::motion(&text, cursor, key).filter(|_| key != "g") {
            select(to..to, cx);
            return true;
        }
        let line_end = vim::motion(&text, cursor, "$").unwrap_or(cursor);
        match key {
            "g" | "d" | "y" | "c" => self.vim_pending = key.chars().next(),
            "i" => self.set_vim_mode(VimMode::Insert, cx),
            "a" => {
                let to = vim::char_after(&text, cursor).min(line_end);
                select(to..to, cx);
                self.set_vim_mode(VimMode::Insert, cx);
            }
            "I" | "A" => {
                let to = vim::motion(&text, cursor, if key == "I" { "^" } else { "$" });
                let to = to.unwrap_or(cursor);
                select(to..to, cx);
                self.set_vim_mode(VimMode::Insert, cx);
            }
            "o" | "O" => {
                let to = vim::motion(&text, cursor, if key == "o" { "$" } else { "0" });
                let to = to.unwrap_or(cursor);
                select(to..to, cx);
                window.dispatch_action(
                    Box::new(act::Enter {
                        secondary: false,
                        shift: false,
                    }),
                    cx,
                );
                if key == "O" {
                    window.dispatch_action(Box::new(act::MoveUp), cx);
                }
                self.set_vim_mode(VimMode::Insert, cx);
            }
            "v" => {
                (self.vim_anchor, self.vim_head) = (cursor, cursor);
                select(cursor..vim::char_after(&text, cursor).min(line_end), cx);
                self.set_vim_mode(VimMode::Visual, cx);
            }
            "x" => {
                let r = cursor..vim::char_after(&text, cursor).min(line_end);
                self.vim_operate('d', r, &text, &editor, window, cx);
            }
            "D" | "C" => {
                let op = if key == "D" { 'd' } else { 'c' };
                self.vim_operate(op, cursor..line_end, &text, &editor, window, cx);
            }
            "p" | "P" => window.dispatch_action(Box::new(act::Paste), cx),
            "u" => {
                window.dispatch_action(Box::new(act::Undo), cx);
                collapse_after(&editor, window, cx);
            }
            // Other printable keys do nothing in normal mode rather than typing
            _ => return ks.key_char.is_some(),
        }
        true
    }

    /// d / y / c over a range: d and c cut (so p pastes it back), y copies
    fn vim_operate(
        &mut self,
        op: char,
        range: std::ops::Range<usize>,
        text: &str,
        editor: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = range.start;
        if op == 'y' {
            cx.write_to_clipboard(ClipboardItem::new_string(text[range].to_string()));
            editor.update(cx, |st, cx| st.set_selected_range(start..start, cx));
            return;
        }
        editor.update(cx, |st, cx| st.set_selected_range(range.clone(), cx));
        if !range.is_empty() {
            window.dispatch_action(Box::new(gpui::component::input::Cut), cx);
        }
        if op == 'c' {
            self.set_vim_mode(state::VimMode::Insert, cx);
        }
    }

    /// One size step per ~40px of ⌘ scroll, so a trackpad flick doesn't jump to the limit
    fn zoom_editor_by_scroll(&mut self, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
        self.zoom_scroll += f32::from(e.delta.pixel_delta(px(16.0)).y);
        let steps = (self.zoom_scroll / 40.0).trunc();
        if steps == 0.0 {
            return;
        }
        self.zoom_scroll -= steps * 40.0;
        let size = (self.settings.mono_size() + steps).clamp(
            settings::Settings::MIN_FONT_SIZE,
            settings::Settings::MAX_FONT_SIZE,
        );
        self.change_setting(|s| s.mono_font_size = size, cx);
    }

    /// Escape (arriving as Dismiss) leaves Insert or Visual for Normal; false lets Normal's Escape dismiss as usual
    pub(crate) fn vim_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(Tab::Query {
            vim_mode, editor, ..
        }) = self.state.active_tab()
        else {
            return false;
        };
        let editor = editor.clone();
        match *vim_mode {
            state::VimMode::Normal => {
                self.vim_pending = None;
                false
            }
            state::VimMode::Insert => {
                // Leaving insert steps back onto the last typed character, as Vim does
                window.dispatch_action(Box::new(gpui::component::input::MoveLeft), cx);
                self.set_vim_mode(state::VimMode::Normal, cx);
                true
            }
            state::VimMode::Visual => {
                let head = self.vim_head;
                editor.update(cx, |st, cx| st.set_selected_range(head..head, cx));
                self.set_vim_mode(state::VimMode::Normal, cx);
                true
            }
        }
    }

    fn set_vim_mode(&mut self, mode: state::VimMode, cx: &mut Context<Self>) {
        self.vim_pending = None;
        if let Some(Tab::Query { vim_mode, .. }) = self.state.active_tab_mut() {
            *vim_mode = mode;
        }
        cx.notify();
    }
}

impl Render for QuarryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.applied_translucent != Some(self.theme.translucent) {
            self.applied_translucent = Some(self.theme.translucent);
            blur::set(window, self.theme.translucent);
        }
        if !cx.has_active_drag() {
            self.active_drag_pane = None;
            self.active_drag_tab = None;
            self.active_drag_split_pane = None;
        }
        self.sync_before_render(window, cx);
        let bottom_panel = self.render_bottom_panel(window, cx);
        let kill_confirm = self.render_kill_confirm(cx);
        let tab_switcher = self.render_tab_switcher(cx);
        let settings_modal = self.render_settings_modal(cx);
        let export_dialog = self.render_export_dialog(cx);
        let import_wizard = self.render_import_wizard(cx);
        let params_dialog = self.render_params_dialog(cx);
        let filter_builder = self.render_filter_builder_modal(cx);
        let peek = self.render_peek(cx);
        let prod_confirm = self.render_prod_confirm(cx);
        let close_confirm = self.render_close_confirm(cx);
        let dock_overlay = self.render_dock_overlay(cx);
        // While ⌃Tab is held, the main area previews the highlighted tab; letting go makes it active
        let previewed = self
            .tab_switcher
            .and_then(|i| self.tab_mru.get(i))
            .and_then(|id| self.state.tabs.iter().find(|t| t.id() == id));
        let active_tab = previewed.or(self.state.active_tab()).cloned();
        let (current_session, current_conn) = self.current_connection(cx);
        let t = self.theme.clone();

        let titlebar = self.render_titlebar(window, cx);
        let tab_strip = self.render_tab_strip(cx);
        let center = self.render_center(active_tab.as_ref(), cx);

        let sidebar = Sidebar::render(&self.state, &self.theme, self.dock, cx);
        let tool_rail = self.render_tool_rail(cx);
        let main_layout = self.render_layout(
            tool_rail.into_any_element(),
            sidebar.into_any_element(),
            bottom_panel,
            tab_strip,
            center.into_any_element(),
        );

        div()
            .id("quarry-root")
            .font_family(cx.theme().font_family.clone())
            .track_focus(&self.focus_handle)
            .key_context("Quarry")
            .on_action(cx.listener(Self::toggle_command_bar))
            .on_action(cx.listener(Self::run_query))
            .on_action(cx.listener(Self::run_all))
            .on_action(cx.listener(Self::review_changes))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_bottom_panel))
            .on_action(cx.listener(Self::new_query))
            .on_action(cx.listener(Self::close_tab))
            .on_action(
                cx.listener(|this, _: &TabSwitcherNext, _, cx| this.step_tab_switcher(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &TabSwitcherPrev, _, cx| this.step_tab_switcher(false, cx)),
            )
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, window, cx| {
                if !ev.modifiers.control {
                    this.commit_tab_switcher(window, cx);
                }
            }))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_settings(cx)))
            .on_action(
                cx.listener(|this, _: &NewConnection, window, cx| this.open_add_conn(window, cx)),
            )
            .on_action(cx.listener(Self::handle_close_window))
            .on_action(cx.listener(Self::handle_quit))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            .on_action(cx.listener(|this, _: &ToggleMaximize, _, cx| {
                this.state.toggle_maximize();
                cx.notify();
            }))
            .on_action(cx.listener(Self::toggle_tx_mode))
            .on_action(cx.listener(Self::commit_tx))
            .on_action(cx.listener(Self::rollback_tx))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::increase_font_size))
            .on_action(cx.listener(Self::decrease_font_size))
            .on_action(cx.listener(Self::reset_font_size))
            .on_action(cx.listener(Self::reopen_closed_tab))
            .on_action(cx.listener(|this, _: &PinCurrentTab, _, cx| this.pin_current_tab(cx)))
            .on_action(cx.listener(|this, _: &CommandPalette, window, cx| {
                this.open_command_palette(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SwitchSpace1, window, cx| this.switch_space(0, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace2, window, cx| this.switch_space(1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace3, window, cx| this.switch_space(2, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace4, window, cx| this.switch_space(3, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace5, window, cx| this.switch_space(4, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace6, window, cx| this.switch_space(5, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace7, window, cx| this.switch_space(6, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace8, window, cx| this.switch_space(7, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SwitchSpace9, window, cx| this.switch_space(8, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleSplit, _, cx| this.toggle_split(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleSplitDirection, _, cx| {
                    this.toggle_split_direction(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusNextPane, _, cx| this.swap_split(cx)))
            .on_action(cx.listener(|this, _: &FocusPrevPane, _, cx| this.swap_split(cx)))
            .on_action(cx.listener(|this, _: &SwapSplitPanes, _, cx| this.swap_pane_positions(cx)))
            .on_action(cx.listener(|this, _: &OpenInSplit, window, cx| {
                this.confirm_command_bar_in_split(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExportResults, _, cx| this.open_export(cx)))
            .on_action(cx.listener(|this, _: &Explain, window, cx| {
                this.start_run(RunMode::Explain, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FormatSql, window, cx| this.format_query(window, cx)))
            .on_action(
                cx.listener(|this, _: &SaveFileAs, window, cx| this.save_query(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| this.open_sql_file(window, cx)))
            .on_action(cx.listener(|this, _: &ExplainAnalyze, window, cx| {
                this.start_run(RunMode::ExplainAnalyze, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &grid::OpenFilterBuilder, window, cx| {
                    this.open_filter_builder(None, window, cx);
                }),
            )
            .on_action(cx.listener(Self::menu_up))
            .on_action(cx.listener(Self::menu_down))
            .on_action(cx.listener(Self::select_next_occurrence))
            .on_action(cx.listener(|this, _: &MoveSidebarLeft, window, cx| {
                this.move_sidebar(layout::Side::Left, window, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveSidebarRight, window, cx| {
                this.move_sidebar(layout::Side::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveBottomPanelBottom, window, cx| {
                this.move_bottom_panel(layout::Side::Bottom, window, cx);
            }))
            .on_action(cx.listener(|this, _: &MoveBottomPanelRight, window, cx| {
                this.move_bottom_panel(layout::Side::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ResetLayout, window, cx| {
                this.reset_layout(window, cx);
            }))
            .on_action(cx.listener(|this, _: &MovePaneLeft, _, cx| {
                this.move_pane_left(cx);
            }))
            .on_action(cx.listener(|this, _: &MovePaneRight, _, cx| {
                this.move_pane_right(cx);
            }))
            .on_action(cx.listener(|this, _: &MovePaneUp, _, cx| {
                this.move_pane_up(cx);
            }))
            .on_action(cx.listener(|this, _: &MovePaneDown, _, cx| {
                this.move_pane_down(cx);
            }))
            .on_action(|_: &ReportIssue, _, cx| {
                cx.open_url("https://github.com/thekodeking/quarry/issues/new");
            })
            .on_action(cx.listener(|this, _: &ViewLastCrash, _, cx| {
                this.view_last_crash(cx);
            }))
            // Drop a SQLite file anywhere to open it (C3, U11)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(path) = paths.paths().first() {
                    this.connect_address(&path.to_string_lossy(), window, cx);
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg_window)
            .text_color(t.text_primary)
            .child(titlebar)
            .child(div().flex_1().min_h_0().child(main_layout))
            .child(StatusBar::render(
                &self.state,
                current_session.as_ref(),
                current_conn.as_ref(),
                match self.state.active_tab() {
                    Some(state::Tab::Query { vim_mode, .. }) if self.settings.vim_mode => {
                        Some(*vim_mode)
                    }
                    _ => None,
                },
                match self.state.active_tab() {
                    Some(state::Tab::Query { editor, .. }) => {
                        let ed = editor.read(cx);
                        let text = ed.value();
                        let mut offset = ed.cursor().min(text.len());
                        while offset > 0 && !text.is_char_boundary(offset) {
                            offset -= 1;
                        }
                        let before = &text[..offset];
                        let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
                        let col = before.rsplit('\n').next().map_or(0, |s| s.chars().count()) + 1;
                        Some((line, col))
                    }
                    _ => None,
                },
                !self.connecting.is_empty(),
                self.available_update
                    .as_ref()
                    .map(|u| (u.version.as_str(), self.installing_update)),
                &self.theme,
                cx,
            ))
            // Modals & Overlays (only rendered when active)
            .children(dock_overlay)
            .children(kill_confirm)
            .children(tab_switcher)
            .children(export_dialog)
            .children(import_wizard)
            .children(params_dialog)
            .children(filter_builder)
            .children(peek)
            .children(prod_confirm)
            .children(settings_modal)
            .children(close_confirm)
            .when_some(
                self.grid_menu.as_ref().map(|(p, m, _)| (*p, m.clone())),
                |this, (pos, menu)| {
                    this.child(
                        deferred(anchored().position(pos).snap_to_window().child(menu))
                            .with_priority(1),
                    )
                },
            )
            .when_some(self.state.toast_message.clone(), |this, (msg, error)| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(64.0))
                        .right(px(16.0))
                        .max_w(px(480.0))
                        .px_3()
                        .py_2()
                        .rounded(px(crate::ui::RADIUS_MD))
                        .bg(crate::theme::Theme::solid(t.bg_panel))
                        .border_1()
                        .border_color(if error { t.danger } else { t.border })
                        .shadow_lg()
                        .text_sm()
                        .text_color(t.text_primary)
                        .flex()
                        .items_start()
                        .gap_2()
                        .when(error, |d| {
                            d.child(
                                gpui::component::Icon::new(IconName::CircleX)
                                    .text_color(t.danger)
                                    .flex_shrink_0()
                                    .mt(px(2.0)),
                            )
                        })
                        .child({
                            // Errors arrive as "hint\nraw driver text"; show the raw part quieter
                            let (head, detail) = match msg.split_once('\n') {
                                Some((h, d)) => (h.to_string(), Some(d.to_string())),
                                None => (msg, None),
                            };
                            v_flex().flex_1().min_w_0().gap_0p5().child(head).when_some(
                                detail,
                                |d, raw| {
                                    d.child(div().text_xs().text_color(t.text_muted).child(raw))
                                },
                            )
                        })
                        .when_some(self.toast_reveal.clone(), |this, path| {
                            this.child(
                                div()
                                    .id("toast-reveal")
                                    .text_color(t.accent)
                                    .cursor_pointer()
                                    .on_click(move |_, _, _| export_dialog::reveal(&path))
                                    .child(object_menu::REVEAL),
                            )
                        }),
                )
            })
            .when(self.state.command_bar_open, |this| {
                this.child(CommandBar::render(
                    &self.state,
                    &self.theme,
                    cx,
                    &self.command_input,
                ))
            })
            .when(self.state.review_modal_open, |this| {
                this.child(ReviewModal::render(&self.state, &self.theme, cx))
            })
            .when(self.state.add_conn_modal_open, |this| {
                this.child(AddConnModal::render(self, cx))
            })
            .when_some(self.pending_crash_report.as_ref(), |this, report| {
                this.child(CrashModal::render(self, report, cx))
            })
    }
}

impl QuarryWindow {
    fn render_layout(
        &self,
        tool_rail: AnyElement,
        sidebar: AnyElement,
        bottom_panel: AnyElement,
        tab_strip: Option<impl IntoElement>,
        center: AnyElement,
    ) -> AnyElement {
        let editor_pane =
            resizable_panel().child(v_flex().size_full().children(tab_strip).child(center));
        let content_split = match self.dock.bottom_panel {
            layout::Side::Bottom => v_resizable("content-split:bottom")
                .on_resize(layout::remember("content-split:bottom"))
                .child(editor_pane)
                .child(
                    resizable_panel()
                        .size(layout::size("content-split:bottom", 1, 280.0))
                        .size_range(px(120.0)..px(900.0))
                        .visible(self.state.bottom_panel_open)
                        .child(bottom_panel),
                )
                .into_any_element(),
            layout::Side::Right => h_resizable("content-split:right")
                .on_resize(layout::remember("content-split:right"))
                .child(editor_pane)
                .child(
                    resizable_panel()
                        .size(layout::size("content-split:right", 1, 380.0))
                        .size_range(px(200.0)..px(900.0))
                        .visible(self.state.bottom_panel_open)
                        .child(bottom_panel),
                )
                .into_any_element(),
            layout::Side::Left | layout::Side::Top => unreachable!(),
        };

        let split = match self.dock.sidebar {
            layout::Side::Left => h_resizable("main-split:sidebar-left")
                .on_resize(layout::remember("main-split:sidebar-left"))
                .child(
                    resizable_panel()
                        .size(layout::size("main-split:sidebar-left", 0, 240.0))
                        .size_range(px(160.0)..px(480.0))
                        .visible(self.state.sidebar_visible)
                        .child(sidebar),
                )
                .child(resizable_panel().child(content_split))
                .into_any_element(),
            layout::Side::Right => h_resizable("main-split:sidebar-right")
                .on_resize(layout::remember("main-split:sidebar-right"))
                .child(resizable_panel().child(content_split))
                .child(
                    resizable_panel()
                        .size(layout::size("main-split:sidebar-right", 1, 240.0))
                        .size_range(px(160.0)..px(480.0))
                        .visible(self.state.sidebar_visible)
                        .child(sidebar),
                )
                .into_any_element(),
            layout::Side::Bottom | layout::Side::Top => unreachable!(),
        };

        let content = div().flex_1().min_w_0().min_h_0().child(split);
        if self.dock.sidebar == layout::Side::Left {
            h_flex()
                .size_full()
                .child(tool_rail)
                .child(content)
                .into_any_element()
        } else {
            h_flex()
                .size_full()
                .child(content)
                .child(tool_rail)
                .into_any_element()
        }
    }

    pub(crate) fn move_sidebar(
        &mut self,
        side: layout::Side,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dock.sidebar != side {
            self.dock.sidebar = side;
            layout::set_dock(self.dock);
            cx.notify();
        }
    }

    pub(crate) fn move_bottom_panel(
        &mut self,
        side: layout::Side,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dock.bottom_panel != side {
            self.dock.bottom_panel = side;
            layout::set_dock(self.dock);
            cx.notify();
        }
    }

    pub(crate) fn reset_layout(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        layout::reset();
        self.dock = layout::dock();
        self.state.sidebar_visible = true;
        self.state.bottom_panel_open = true;
        cx.notify();
    }

    pub(crate) fn open_sidebar_dock_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.dock.sidebar;
        let choices = vec![
            (
                "Move to Left".to_string(),
                current == layout::Side::Left,
                layout::Side::Left,
            ),
            (
                "Move to Right".to_string(),
                current == layout::Side::Right,
                layout::Side::Right,
            ),
        ];
        self.open_choice_menu(choices, Self::apply_sidebar_dock, window, cx);
    }

    pub(crate) fn open_bottom_panel_dock_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.dock.bottom_panel;
        let choices = vec![
            (
                "Move to Bottom".to_string(),
                current == layout::Side::Bottom,
                layout::Side::Bottom,
            ),
            (
                "Move to Right".to_string(),
                current == layout::Side::Right,
                layout::Side::Right,
            ),
        ];
        self.open_choice_menu(choices, Self::apply_bottom_panel_dock, window, cx);
    }

    fn apply_sidebar_dock(&mut self, side: layout::Side, cx: &mut Context<Self>) {
        self.dock.sidebar = side;
        layout::set_dock(self.dock);
        cx.notify();
    }

    fn apply_bottom_panel_dock(&mut self, side: layout::Side, cx: &mut Context<Self>) {
        self.dock.bottom_panel = side;
        layout::set_dock(self.dock);
        cx.notify();
    }

    fn render_dock_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !cx.has_active_drag() {
            return None;
        }

        let is_pane_drag = self.active_drag_pane.is_some();
        let is_tab_drag = self.active_drag_tab.is_some();
        let is_split_drag = self.active_drag_split_pane.is_some();
        if !is_pane_drag && !is_tab_drag && !is_split_drag {
            return None;
        }

        // Drop zones tint with the theme accent
        let accent = self.theme.accent;
        let zone = move |a: f32| Rgba { a, ..accent };

        let mut overlay = div()
            .id("dock-overlay")
            .absolute()
            .inset_0()
            .top(px(crate::ui::BAR_H))
            .bottom(px(crate::ui::SMALL_H))
            .on_drop(cx.listener(|this, _: &layout::DraggedPane, _, cx| {
                this.active_drag_pane = None;
                cx.notify();
            }))
            .on_drop(cx.listener(|this, _: &layout::DraggedTab, _, cx| {
                this.active_drag_tab = None;
                cx.notify();
            }))
            .on_drop(cx.listener(|this, _: &layout::DraggedSplitPane, _, cx| {
                this.active_drag_split_pane = None;
                cx.notify();
            }));

        let zone_label = |icon: IconName, text: &'static str| {
            h_flex()
                .px_3()
                .py_1p5()
                .gap_1p5()
                .items_center()
                .rounded(px(crate::ui::RADIUS_MD))
                .bg(self.theme.bg_panel)
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(zone(1.0))
                .child(Icon::new(icon).xsmall().text_color(zone(1.0)))
                .child(text)
        };

        if self.active_drag_pane == Some(layout::Pane::Sidebar) {
            overlay = overlay
                .child(
                    div()
                        .id("drop-zone-sidebar-left")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .w_1_4()
                        .min_w(px(200.0))
                        .max_w(px(360.0))
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_r_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedPane>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(cx.listener(|this, _: &layout::DraggedPane, window, cx| {
                            this.active_drag_pane = None;
                            this.move_sidebar(layout::Side::Left, window, cx);
                        }))
                        .child(zone_label(IconName::ArrowLeft, "Dock to Left")),
                )
                .child(
                    div()
                        .id("drop-zone-sidebar-right")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w_1_4()
                        .min_w(px(200.0))
                        .max_w(px(360.0))
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_l_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedPane>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(cx.listener(|this, _: &layout::DraggedPane, window, cx| {
                            this.active_drag_pane = None;
                            this.move_sidebar(layout::Side::Right, window, cx);
                        }))
                        .child(zone_label(IconName::ArrowRight, "Dock to Right")),
                );
        } else if self.active_drag_pane == Some(layout::Pane::BottomPanel) {
            overlay = overlay
                .child(
                    div()
                        .id("drop-zone-bottom-bottom")
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h_1_4()
                        .min_h(px(140.0))
                        .max_h(px(320.0))
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_t_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedPane>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(cx.listener(|this, _: &layout::DraggedPane, window, cx| {
                            this.active_drag_pane = None;
                            this.move_bottom_panel(layout::Side::Bottom, window, cx);
                        }))
                        .child(zone_label(IconName::ArrowDown, "Dock to Bottom")),
                )
                .child(
                    div()
                        .id("drop-zone-bottom-right")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w_1_4()
                        .min_w(px(200.0))
                        .max_w(px(360.0))
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_l_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedPane>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(cx.listener(|this, _: &layout::DraggedPane, window, cx| {
                            this.active_drag_pane = None;
                            this.move_bottom_panel(layout::Side::Right, window, cx);
                        }))
                        .child(zone_label(IconName::ArrowRight, "Dock to Right")),
                );
        } else if let Some(from_ix) = self.active_drag_split_pane {
            overlay = overlay
                .child(
                    div()
                        .id("drop-zone-split-left")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .w_1_4()
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_r_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedSplitPane>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(
                            cx.listener(move |this, _: &layout::DraggedSplitPane, _, cx| {
                                this.active_drag_split_pane = None;
                                this.move_split_pane_to_side(from_ix, layout::Side::Left, cx);
                            }),
                        )
                        .child(zone_label(IconName::ArrowLeft, "Dock to Left")),
                )
                .child(
                    div()
                        .id("drop-zone-split-right")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w_1_4()
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_l_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedSplitPane>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(
                            cx.listener(move |this, _: &layout::DraggedSplitPane, _, cx| {
                                this.active_drag_split_pane = None;
                                this.move_split_pane_to_side(from_ix, layout::Side::Right, cx);
                            }),
                        )
                        .child(zone_label(IconName::ArrowRight, "Dock to Right")),
                )
                .child(
                    div()
                        .id("drop-zone-split-top")
                        .absolute()
                        .top_0()
                        .h_1_2()
                        .left_1_4()
                        .right_1_4()
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_b_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedSplitPane>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(
                            cx.listener(move |this, _: &layout::DraggedSplitPane, _, cx| {
                                this.active_drag_split_pane = None;
                                this.move_split_pane_to_side(from_ix, layout::Side::Top, cx);
                            }),
                        )
                        .child(zone_label(IconName::ArrowUp, "Dock to Top")),
                )
                .child(
                    div()
                        .id("drop-zone-split-bottom")
                        .absolute()
                        .bottom_0()
                        .h_1_2()
                        .left_1_4()
                        .right_1_4()
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_t_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedSplitPane>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(
                            cx.listener(move |this, _: &layout::DraggedSplitPane, _, cx| {
                                this.active_drag_split_pane = None;
                                this.move_split_pane_to_side(from_ix, layout::Side::Bottom, cx);
                            }),
                        )
                        .child(zone_label(IconName::ArrowDown, "Dock to Bottom")),
                );
        } else if self.active_drag_tab.is_some() {
            overlay = overlay
                .child(
                    div()
                        .id("drop-zone-tab-split-left")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .w_1_4()
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_r_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedTab>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(cx.listener(
                            |this, &layout::DraggedTab(idx): &layout::DraggedTab, _, cx| {
                                this.active_drag_tab = None;
                                if let Some(tab) = this.state.tabs.get(idx) {
                                    let id = tab.id().to_string();
                                    this.open_tab_in_split_side(id, layout::Side::Left, cx);
                                }
                            },
                        ))
                        .child(zone_label(IconName::ArrowLeft, "Dock to Left")),
                )
                .child(
                    div()
                        .id("drop-zone-tab-split-right")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w_1_4()
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_l_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedTab>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(cx.listener(
                            |this, &layout::DraggedTab(idx): &layout::DraggedTab, _, cx| {
                                this.active_drag_tab = None;
                                if let Some(tab) = this.state.tabs.get(idx) {
                                    let id = tab.id().to_string();
                                    this.open_tab_in_split_side(id, layout::Side::Right, cx);
                                }
                            },
                        ))
                        .child(zone_label(IconName::ArrowRight, "Dock to Right")),
                )
                .child(
                    div()
                        .id("drop-zone-tab-split-top")
                        .absolute()
                        .top_0()
                        .h_1_2()
                        .left_1_4()
                        .right_1_4()
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_b_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedTab>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(cx.listener(
                            |this, &layout::DraggedTab(idx): &layout::DraggedTab, _, cx| {
                                this.active_drag_tab = None;
                                if let Some(tab) = this.state.tabs.get(idx) {
                                    let id = tab.id().to_string();
                                    this.open_tab_in_split_side(id, layout::Side::Top, cx);
                                }
                            },
                        ))
                        .child(zone_label(IconName::ArrowUp, "Dock to Top")),
                )
                .child(
                    div()
                        .id("drop-zone-tab-split-bottom")
                        .absolute()
                        .bottom_0()
                        .h_1_2()
                        .left_1_4()
                        .right_1_4()
                        .bg(zone(0.10))
                        .border_1()
                        .border_dashed()
                        .border_color(zone(0.45))
                        .rounded_t_lg()
                        .flex()
                        .items_center()
                        .justify_center()
                        .drag_over::<layout::DraggedTab>(move |style, _, _, _| {
                            style.bg(zone(0.22)).border_color(zone(1.0))
                        })
                        .on_drop(cx.listener(
                            |this, &layout::DraggedTab(idx): &layout::DraggedTab, _, cx| {
                                this.active_drag_tab = None;
                                if let Some(tab) = this.state.tabs.get(idx) {
                                    let id = tab.id().to_string();
                                    this.open_tab_in_split_side(id, layout::Side::Bottom, cx);
                                }
                            },
                        ))
                        .child(zone_label(IconName::ArrowDown, "Dock to Bottom")),
                );
        }

        Some(overlay.into_any_element())
    }
}

fn main() {
    if std::env::var("QUARRY_SSH_ASKPASS").is_ok() {
        let prompt = std::env::args().nth(1).unwrap_or_default().to_lowercase();
        let secret = if prompt.contains("passphrase") {
            std::env::var("QUARRY_SSH_PASSPHRASE")
                .or_else(|_| std::env::var("QUARRY_SSH_SECRET"))
                .unwrap_or_default()
        } else {
            std::env::var("QUARRY_SSH_PASSWORD")
                .or_else(|_| std::env::var("QUARRY_SSH_SECRET"))
                .unwrap_or_default()
        };
        println!("{secret}");
        return;
    }
    crate::ssh::init_ssh_env();

    crash_reporter::init();
    crash_reporter::add_breadcrumb("Quarry process started");

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .compact()
        .try_init();

    tracing::info!("Starting Quarry v{}...", env!("CARGO_PKG_VERSION"));

    gpui::application()
        .with_assets(gpui::assets::AllAssets)
        .run(|cx: &mut App| {
            gpui::init(cx);
            // Quarry's own palette is dark-only for now; keep gpui-kit widgets in sync
            gpui::component::theme::Theme::change(
                gpui::component::theme::ThemeMode::Dark,
                None,
                cx,
            );
            theme::load_fonts(cx);
            menus::bind_keys(cx);
            ui::init_modal_focus(cx);
            // gpui renders animations at their end state when this is set; it doesn't read the OS itself
            // ponytail: read once at launch, toggling Reduce Motion needs a restart
            cx.set_reduce_motion(ui::os_reduce_motion());
            grid::bind_keys(cx);
            menus::set_app_menus(cx);
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: None, // Custom unified titlebar with cross-platform OS traffic light offset
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.0), px(12.0))),
                }),
                ..Default::default()
            };

            gpui::open_window(options, cx, |window, cx| {
                let entity = cx.new(|cx| {
                    // Starts with the onboarding input focused, so a pasted URL just works
                    QuarryWindow::new(window, cx)
                });
                let weak = entity.downgrade();
                window.on_window_should_close(cx, move |_window, cx| {
                    weak.update(cx, |this, cx| {
                        if this.force_close {
                            true
                        } else if this.has_unsaved_changes(cx) {
                            this.confirm_close_open = true;
                            cx.notify();
                            false
                        } else {
                            true
                        }
                    })
                    .unwrap_or(true)
                });
                entity
            })
            .unwrap();
        });
}

fn os_is_dark(window: &Window) -> bool {
    matches!(
        window.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}
