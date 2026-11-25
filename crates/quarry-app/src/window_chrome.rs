// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! The window frame around the tabs: title bar, tab strip and the centre area

use super::*;

impl QuarryWindow {
    /// Keeps derived state (theme, settings, lint, tab connections) in step before drawing
    pub(crate) fn sync_before_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let changes = self
            .config_watcher
            .poll_changes(std::time::Duration::from_millis(500));
        if changes.settings_changed {
            self.reload_settings(cx);
        }
        if changes.keymap_changed {
            let _ = crate::settings::apply_keymap(cx);
        }

        self.sync_theme(window, cx);
        if self.applied_settings.as_ref() != Some(&self.settings) {
            self.apply_settings_to_views(window, cx);
        }
        // The schema changed (connect, switch, refresh): squiggles must reflect the new one
        self.load_schema_objects(window, cx);
        if let Some(id) = self.state.pending_compare.take() {
            self.compare_schema(&id, window, cx);
        }
        if self.state.linted_gen != self.state.catalog_gen {
            self.state.linted_gen = self.state.catalog_gen;
            let editors: Vec<_> = self
                .state
                .tabs
                .iter()
                .filter_map(|t| match t {
                    Tab::Query { editor, .. } => Some(editor.clone()),
                    _ => None,
                })
                .collect();
            let catalog = self.state.completion_catalog.clone();
            cx.defer(move |cx| {
                editors
                    .iter()
                    .for_each(|e| completion::lint_editor(e, &catalog, cx))
            });
        }
        self.sync_tab_connection(window, cx);
        self.reopen_pending_table_tabs(window, cx);
        self.track_tab_mru(window, cx);
    }

    /// Where you are, and the one box that does everything (⌘K)
    pub(crate) fn render_titlebar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        // macOS: clear the traffic lights; elsewhere a normal gutter
        #[cfg(target_os = "macos")]
        let titlebar_left_pad = px(88.0);
        #[cfg(not(target_os = "macos"))]
        let titlebar_left_pad = px(12.0);
        h_flex()
            .h(px(crate::ui::BAR_H))
            .w_full()
            .pl(titlebar_left_pad)
            .when(!cfg!(target_os = "windows"), |d| d.pr_3())
            .bg(t.bg_sidebar)
            .border_b_1()
            .border_color(t.border)
            .items_center()
            // Equal zero-basis sides keep the omnibox centred on the window, whatever the breadcrumb says
            .child(
                h_flex()
                    .flex_1()
                    .flex_basis(px(0.0))
                    .min_w_0()
                    .h_full()
                    .pr_3()
                    // Outside the drag area: Windows would take clicks on it as title bar drags
                    .children(self.menu_bar.clone().map(|m| div().flex_shrink_0().mr_2().child(m)))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag)
                            .children(self.render_breadcrumb()),
                    ),
            )
            .child(
                h_flex()
                    .id("omnibox-trigger-btn")
                    // Shrinks before it overlaps the breadcrumb, never below a usable width
                    .w(px(360.0))
                    .min_w(px(180.0))
                    .flex_shrink(1.0)
                    .h(px(crate::ui::ROW_H))
                    .px_2p5()
                    .gap_2()
                    .rounded(px(crate::ui::RADIUS_MD))
                    .bg(t.bg_panel)
                    .border_1()
                    .border_color(t.border)
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.bg_hover))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_command_bar(&ToggleCommandBar, window, cx)
                    }))
                    .child(
                        Icon::new(IconName::Search)
                            .xsmall()
                            .text_color(t.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(t.text_muted)
                            .child("Commands, tables, SQL…"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_muted)
                            .flex_shrink_0()
                            .child(crate::ui::format_keys("⌘K")),
                    ),
            )
            // Mirrors the traffic-light gutter on the left, with quick workspace actions on the right
            .child(
                h_flex()
                    .flex_1()
                    .flex_basis(px(0.0))
                    .min_w_0()
                    .h_full()
                    .ml(titlebar_left_pad - px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_0p5()
                            .pr_1()
                            .child(
                                zbutton("titlebar-new-query")
                                    .icon(IconName::Plus)
                                    .ghost()
                                    .small()
                                    .tooltip(crate::ui::format_keys("New query (⌘N)"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.new_query(&NewQuery, window, cx)
                                    })),
                            )
                            .child(
                                zbutton("titlebar-toggle-sidebar")
                                    .icon(IconName::Columns2)
                                    .ghost()
                                    .small()
                                    .when(self.state.sidebar_visible, |b| b.text_color(t.accent))
                                    .tooltip(crate::ui::format_keys("Toggle sidebar (⌘B)"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_sidebar(&ToggleSidebar, window, cx)
                                    })),
                            )
                            .child(
                                zbutton("titlebar-toggle-panel")
                                    .icon(if self.state.bottom_panel_open {
                                        IconName::PanelTopClose
                                    } else {
                                        IconName::PanelTopOpen
                                    })
                                    .ghost()
                                    .small()
                                    .when(self.state.bottom_panel_open, |b| b.text_color(t.accent))
                                    .tooltip(crate::ui::format_keys("Toggle panel (⌘J)"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_bottom_panel(&ToggleBottomPanel, window, cx)
                                    })),
                            )
                            .child(
                                zbutton("titlebar-settings")
                                    .icon(IconName::Settings2)
                                    .ghost()
                                    .small()
                                    .tooltip(crate::ui::format_keys("Settings (⌘,)"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_settings(cx)
                                    })),
                            ),
                    )
                    .when(cfg!(target_os = "windows"), |d| {
                        d.children(self.render_caption_buttons(window))
                    }),
            )
    }

    /// Slim JetBrains/Zed-style outer tool-window rail for sidebar & bottom panel views
    pub(crate) fn render_tool_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let is_right = self.dock.sidebar == layout::Side::Right;
        let rail_btn = |id: &'static str, icon: IconName, active: bool, tip: &'static str| {
            div()
                .id(id)
                .relative()
                .size(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(crate::ui::RADIUS_MD))
                .cursor_pointer()
                .when(active, |d| d.bg(t.bg_selected).text_color(t.accent))
                .when(!active, |d| {
                    d.text_color(t.text_muted)
                        .hover(|s| s.bg(t.bg_hover).text_color(t.text_primary))
                })
                .tooltip(move |window, cx| {
                    gpui::component::tooltip::Tooltip::new(crate::ui::format_keys(tip))
                        .build(window, cx)
                })
                .when(active, |d| {
                    d.child(
                        div()
                            .absolute()
                            .when(!is_right, |b| b.left(px(-5.0)))
                            .when(is_right, |b| b.right(px(-5.0)))
                            .top(px(6.0))
                            .bottom(px(6.0))
                            .w(px(2.0))
                            .rounded_full()
                            .bg(t.accent),
                    )
                })
                .child(Icon::new(icon).size(px(16.0)))
        };

        v_flex()
            .w(px(42.0))
            .h_full()
            .flex_shrink_0()
            .py_2()
            .items_center()
            .justify_between()
            .bg(t.bg_sidebar)
            .when(is_right, |d| d.border_l_1())
            .when(!is_right, |d| d.border_r_1())
            .border_color(t.border)
            .child(
                v_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        rail_btn(
                            "rail-connections",
                            IconName::Database,
                            self.state.sidebar_visible
                                && self.state.sidebar_tab == crate::state::SidebarTab::Connections,
                            "Database Explorer (⌘B)",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.state.sidebar_visible
                                && this.state.sidebar_tab
                                    == crate::state::SidebarTab::Connections
                            {
                                this.state.sidebar_visible = false;
                            } else {
                                this.state.sidebar_visible = true;
                                this.state.sidebar_tab = crate::state::SidebarTab::Connections;
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        rail_btn(
                            "rail-queries",
                            IconName::FileCode,
                            self.state.sidebar_visible
                                && self.state.sidebar_tab == crate::state::SidebarTab::Queries,
                            "Saved Queries",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.state.sidebar_visible
                                && this.state.sidebar_tab == crate::state::SidebarTab::Queries
                            {
                                this.state.sidebar_visible = false;
                            } else {
                                this.state.sidebar_visible = true;
                                this.state.sidebar_tab = crate::state::SidebarTab::Queries;
                            }
                            cx.notify();
                        })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .items_center()
                    .child(
                        rail_btn(
                            "rail-output",
                            IconName::SquareTerminal,
                            self.state.bottom_panel_open
                                && self.state.bottom_panel_tab
                                    == crate::state::BottomPanelTab::Messages,
                            "Output (⌘J)",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_panel_tab(crate::state::BottomPanelTab::Messages, cx)
                        })),
                    )
                    .child(
                        rail_btn(
                            "rail-history",
                            IconName::ScrollText,
                            self.state.bottom_panel_open
                                && self.state.bottom_panel_tab
                                    == crate::state::BottomPanelTab::History,
                            "Query History",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_panel_tab(crate::state::BottomPanelTab::History, cx)
                        })),
                    )
                    .child(
                        rail_btn(
                            "rail-logs",
                            IconName::ListFilter,
                            self.state.bottom_panel_open
                                && self.state.bottom_panel_tab
                                    == crate::state::BottomPanelTab::Logs,
                            "Activity Log",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_panel_tab(crate::state::BottomPanelTab::Logs, cx)
                        })),
                    )
                    .child(
                        rail_btn(
                            "rail-server",
                            IconName::Server,
                            self.state.bottom_panel_open
                                && self.state.bottom_panel_tab
                                    == crate::state::BottomPanelTab::Server,
                            "Server Monitor",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_panel_tab(crate::state::BottomPanelTab::Server, cx)
                        })),
                    ),
            )
    }

    fn toggle_panel_tab(&mut self, tab: crate::state::BottomPanelTab, cx: &mut Context<Self>) {
        if self.state.bottom_panel_open && self.state.bottom_panel_tab == tab {
            self.state.bottom_panel_open = false;
        } else {
            self.state.bottom_panel_open = true;
            self.state.bottom_panel_tab = tab;
        }
        cx.notify();
    }

    /// Windows draws no frame around our title bar: min / max / close, clicked through to the OS
    /// by their control area (close still goes through the unsaved-changes check)
    fn render_caption_buttons(&self, window: &Window) -> [impl IntoElement; 3] {
        let t = &self.theme;
        let max_icon = if window.is_maximized() {
            IconName::WindowRestore
        } else {
            IconName::WindowMaximize
        };
        [
            ("caption-min", IconName::WindowMinimize, WindowControlArea::Min),
            ("caption-max", max_icon, WindowControlArea::Max),
            ("caption-close", IconName::WindowClose, WindowControlArea::Close),
        ]
        .map(|(id, icon, area)| {
            let close = matches!(area, WindowControlArea::Close);
            div()
                .id(id)
                .w(px(46.0))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.text_muted)
                .hover(|s| {
                    if close {
                        s.bg(gpui::rgb(0xc42b1c)).text_color(gpui::white())
                    } else {
                        s.bg(t.bg_hover)
                    }
                })
                .window_control_area(area)
                .child(Icon::new(icon).small())
        })
    }

    /// Where the active tab points: `● connection › schema › ▦ table`.
    /// The database and full address are in the tooltip; the crumbs stay short.
    fn render_breadcrumb(&self) -> Option<impl IntoElement> {
        let t = &self.theme;
        let (conn, schema, table) = match self.state.active_tab()? {
            Tab::Table {
                connection,
                schema_name,
                table_name,
                ..
            } => (
                connection.as_ref()?,
                Some(schema_name.clone()),
                Some(table_name.clone()),
            ),
            Tab::Query { schema, .. } => {
                (self.state.active_connection.as_ref()?, schema.clone(), None)
            }
        };
        let env_color = match conn.env {
            Environment::Local => t.success,
            Environment::Staging => t.warning,
            Environment::Prod => t.danger,
        };
        let sep = || div().flex_shrink_0().text_color(t.border).child("/");
        let tip = crate::connections::display_address(conn);
        Some(
            h_flex()
                .id("breadcrumb")
                .items_center()
                .gap_2()
                .min_w_0()
                .overflow_hidden()
                .text_sm()
                .text_color(t.text_muted)
                .tooltip(move |window, cx| {
                    gpui::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                // Connection: env dot + name, truncates last-but-one
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .min_w(px(48.0))
                        .child(
                            div()
                                .flex_shrink_0()
                                .size(px(6.0))
                                .rounded_full()
                                .bg(env_color),
                        )
                        .child(div().truncate().child(conn.name.clone())),
                )
                .children(schema.map(|s| {
                    h_flex()
                        .gap_2()
                        .items_center()
                        .min_w(px(32.0))
                        .child(sep())
                        .child(div().truncate().child(s))
                }))
                .children(table.map(|name| {
                    h_flex()
                        .items_center()
                        .gap_2()
                        .flex_shrink_0()
                        .max_w(px(240.0))
                        .child(sep())
                        .child(
                            Icon::new(IconName::Table)
                                .xsmall()
                                .text_color(t.text_secondary),
                        )
                        .child(
                            div()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.text_primary)
                                .child(name),
                        )
                })),
        )
    }

    /// Icon + title + close per tab; hidden when nothing is open
    pub(crate) fn render_tab_strip(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let t = &self.theme;
        (!self.state.tabs.is_empty()).then(|| {
            h_flex()
                .id("tab-strip-bar")
                .h(px(BAR_H))
                .w_full()
                .px_1()
                .gap_px()
                .bg(t.bg_panel)
                .border_b_1()
                .border_color(t.border)
                .items_end()
                .drag_over::<layout::DraggedTab>(move |style, _, _, _| style)
                .on_drop(cx.listener(|this, &layout::DraggedTab(from_idx): &layout::DraggedTab, _, cx| {
                    this.active_drag_tab = None;
                    let last = this.state.tabs.len().saturating_sub(1);
                    this.reorder_tab(from_idx, last, cx);
                }))
                .children(self.state.tabs.iter().enumerate().map(|(idx, tab)| {
                    let is_active = idx == self.state.active_tab_index;
                    let icon = match tab {
                        Tab::Query { .. } => IconName::SquareTerminal,
                        Tab::Table { .. } => IconName::Table,
                    };
                    // Table tabs keep their connection: mark staging/prod so a risky tab is obvious at a glance
                    let env_color = match tab {
                        Tab::Table { connection: Some(c), .. } => match c.env {
                            Environment::Prod => Some(t.danger),
                            Environment::Staging => Some(t.warning),
                            Environment::Local => None,
                        },
                        _ => None,
                    };
                    let table_obj = match tab {
                        Tab::Table { schema_name, table_name, .. } => {
                            Some(object_menu::DbObject::Table { schema: schema_name.clone(), table: table_name.clone() })
                        }
                        _ => None,
                    };
                    let this_entity = cx.entity().clone();
                    let tab_title = tab.title().to_string();
                    let bg_hover = t.bg_hover;
                    let accent = t.accent;
                    h_flex()
                        .id(("tab-select", idx))
                        .h(px(CONTROL_H))
                        .pl_2p5()
                        .pr_1()
                        .gap_1p5()
                        .items_center()
                        .rounded_t(px(crate::ui::RADIUS_SM))
                        .cursor_pointer()
                        .group("tab-row")
                        .relative()
                        // Active tab: joins the content below and carries a JetBrains-style bottom accent bar
                        .when(is_active, |d| {
                            d.bg(t.bg_app).child(
                                div()
                                    .absolute()
                                    .bottom_0()
                                    .left_1()
                                    .right_1()
                                    .h(px(2.0))
                                    .rounded_full()
                                    .bg(accent),
                            )
                        })
                        .when(!is_active, |d| d.hover(|s| s.bg(t.bg_hover)))
                        .on_drag(layout::DraggedTab(idx), {
                            let this_entity = this_entity.clone();
                            let tab_title = tab_title.clone();
                            move |_, _, _, cx| {
                                this_entity.update(cx, |this, _| {
                                    this.active_drag_tab = Some(idx);
                                });
                                cx.new(|_| DragGhost::new(tab_title.clone()))
                            }
                        })
                        .drag_over::<layout::DraggedTab>(move |style, _, _, _| {
                            style.bg(bg_hover).border_b_2().border_color(accent)
                        })
                        .on_drop(cx.listener(move |this, &layout::DraggedTab(from_idx): &layout::DraggedTab, _, cx| {
                            this.active_drag_tab = None;
                            this.reorder_tab(from_idx, idx, cx);
                        }))
                        .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab_at(idx, cx);
                            this.refocus_root(window, cx);
                        }))
                        .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                            this.state.active_tab_index = idx;
                            // Double-clicking a preview tab keeps it
                            if e.click_count() >= 2 {
                                this.preview_tab = None;
                            }
                            cx.notify();
                        }))
                        .when_some(table_obj, |d, obj| {
                            d.on_mouse_down(MouseButton::Right, cx.listener(move |this, _, window, cx| {
                                this.state.active_tab_index = idx;
                                this.open_object_menu(obj.clone(), window, cx);
                            }))
                        })
                        .when_some(env_color, |d, c| {
                            d.child(div().size(px(6.0)).rounded_full().bg(c).flex_shrink_0())
                        })
                        .child(Icon::new(icon).xsmall().text_color(if is_active { t.accent } else { t.text_muted }))
                        .child(
                            div()
                                .text_sm()
                                // A preview tab (replaced by the next click) reads a step quieter; italic
                                // where the UI font has one
                                .text_color(match (is_active, self.preview_tab.as_deref() == Some(tab.id())) {
                                    (true, false) => t.text_primary,
                                    (false, true) => t.text_muted,
                                    _ => t.text_secondary,
                                })
                                .when(self.preview_tab.as_deref() == Some(tab.id()), |d| d.italic())
                                .child(tab.title().to_string()),
                        )
                        .when(
                            self.split_id.as_deref() == Some(tab.id()) && self.split_tab().is_some(),
                            |d| {
                                d.child(
                                    div()
                                        .px_1()
                                        .py(px(1.0))
                                        .rounded(px(crate::ui::RADIUS_SM))
                                        .bg(t.accent.opacity(0.14))
                                        .text_color(t.accent)
                                        .child(
                                            Icon::new(
                                                if layout::split_direction()
                                                    == layout::SplitDirection::Horizontal
                                                {
                                                    IconName::Rows2
                                                } else {
                                                    IconName::Columns2
                                                },
                                            )
                                            .size(px(10.0)),
                                        ),
                                )
                            },
                        )
                        .when(
                            // Unsaved: a new tab with text in it, or a file that differs from disk
                            matches!(tab, Tab::Query { editor, saved_text, .. } if editor.read(cx).value().as_ref() != saved_text.as_str()),
                            |d| d.child(div().size(px(6.0)).rounded_full().bg(t.text_secondary)),
                        )
                        .child(
                            // Close shows on the active tab and on hover, like Zed
                            div()
                                .when(!is_active, |d| {
                                    d.invisible().group_hover("tab-row", |s| s.visible())
                                })
                                .child(
                                    zbutton(("tab-close", idx))
                                        .icon(IconName::X)
                                        .ghost()
                                        .xsmall()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            // The tab under this button selects itself on click; without this it would
                                            // re-select the closed slot (a neighbour, or nothing when it was the last)
                                            cx.stop_propagation();
                                            this.close_tab_at(idx, cx);
                                            this.refocus_root(window, cx);
                                        })),
                                ),
                        )
                }))
                // Tabs sit on the strip's bottom edge; centre the + on a tab's height so it lines up with them
                .child(
                    h_flex().h(px(CONTROL_H)).px_1().items_center().child(
                        zbutton("tab-add-btn")
                            .icon(IconName::Plus)
                            .ghost()
                            .xsmall()
                            .tooltip(crate::ui::format_keys("New query (⌘N)"))
                            .on_click(cx.listener(|this, _, window, cx| this.new_query(&NewQuery, window, cx))),
                    ),
                )
                // Split toggle at the strip's far end; lit while a split is showing
                .child(div().flex_1())
                .child(
                    h_flex().h(px(CONTROL_H)).px_1().items_center().child(
                        zbutton("tab-split-btn")
                            .icon(if layout::split_direction() == layout::SplitDirection::Horizontal {
                                IconName::Rows2
                            } else {
                                IconName::Columns2
                            })
                            .ghost()
                            .xsmall()
                            .when(self.split_tab().is_some(), |b| b.text_color(t.accent))
                            .tooltip(crate::ui::format_keys("Split view (⌘\\)"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_split(cx))),
                    ),
                )
        })
    }

    fn render_split_pane(
        &mut self,
        tab: &Tab,
        is_active: bool,
        is_second_pane: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme.clone();
        let icon = match tab {
            Tab::Query { .. } => IconName::SquareTerminal,
            Tab::Table { .. } => IconName::Table,
        };
        let env_color = match tab {
            Tab::Table {
                connection: Some(c),
                ..
            } => match c.env {
                Environment::Prod => Some(t.danger),
                Environment::Staging => Some(t.warning),
                Environment::Local => None,
            },
            _ => None,
        };
        let table_obj = match tab {
            Tab::Table {
                schema_name,
                table_name,
                ..
            } => Some(object_menu::DbObject::Table {
                schema: schema_name.clone(),
                table: table_name.clone(),
            }),
            _ => None,
        };
        let is_modified = matches!(
            tab,
            Tab::Query { editor, saved_text, .. }
                if editor.read(cx).value().as_ref() != saved_text.as_str()
        );
        let is_horizontal = layout::split_direction() == layout::SplitDirection::Horizontal;
        let dir_icon = if is_horizontal {
            IconName::Columns2
        } else {
            IconName::Rows2
        };
        let dir_tooltip = if is_horizontal {
            "Split vertically (⌥⌘\\)"
        } else {
            "Split horizontally (⌥⌘\\)"
        };
        let p_ix: usize = if is_second_pane { 1 } else { 0 };
        let this_entity = cx.entity().clone();
        let tab_title = tab.title().to_string();

        let tab_chip = h_flex()
            .id(("split-pane-tab", p_ix))
            .h(px(CONTROL_H))
            .pl_2p5()
            .pr_1()
            .gap_1p5()
            .items_center()
            .rounded_t(px(crate::ui::RADIUS_SM))
            .cursor_pointer()
            .when(is_active, |d| d.bg(t.bg_app))
            .when(!is_active, |d| d.hover(|s| s.bg(t.bg_hover)))
            .on_drag(layout::DraggedSplitPane(p_ix), {
                let this_entity = this_entity.clone();
                let tab_title = tab_title.clone();
                move |_, _, _, cx| {
                    this_entity.update(cx, |this, _| {
                        this.active_drag_split_pane = Some(p_ix);
                    });
                    cx.new(|_| DragGhost::new(tab_title.clone()))
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if !is_active {
                    this.swap_split(cx);
                }
            }))
            .when_some(env_color, |d, c| d.border_b_2().border_color(c))
            .when_some(table_obj, |d, obj| {
                d.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, window, cx| {
                        this.open_object_menu(obj.clone(), window, cx);
                    }),
                )
            })
            .child(Icon::new(icon).xsmall().text_color(if is_active {
                t.accent
            } else {
                t.text_muted
            }))
            .child(
                div()
                    .text_sm()
                    .text_color(if is_active {
                        t.text_primary
                    } else {
                        t.text_secondary
                    })
                    .child(tab.title().to_string()),
            )
            .when(is_modified, |d| {
                d.child(div().size(px(6.0)).rounded_full().bg(t.text_secondary))
            })
            .child(
                zbutton(("split-tab-close", p_ix))
                    .icon(IconName::X)
                    .ghost()
                    .xsmall()
                    .tooltip("Close tab")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_split_pane(is_second_pane, window, cx);
                    })),
            );

        let header = h_flex()
            .id(("split-pane-header", p_ix))
            .h(px(BAR_H))
            .w_full()
            .px_1()
            .gap_px()
            .bg(t.bg_panel)
            .border_b_1()
            .border_color(t.border)
            .items_end()
            .child(tab_chip)
            .child(div().flex_1())
            .child(
                h_flex()
                    .h(px(CONTROL_H))
                    .px_1()
                    .items_center()
                    .gap_1()
                    .when(!is_horizontal && p_ix == 1, |d| {
                        d.child(
                            zbutton(("split-dock-left", p_ix))
                                .icon(IconName::ArrowLeft)
                                .ghost()
                                .xsmall()
                                .tooltip("Dock pane to left (⌥⌘[)")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_split_pane_to_side(1, layout::Side::Left, cx);
                                })),
                        )
                    })
                    .when(!is_horizontal && p_ix == 0, |d| {
                        d.child(
                            zbutton(("split-dock-right", p_ix))
                                .icon(IconName::ArrowRight)
                                .ghost()
                                .xsmall()
                                .tooltip("Dock pane to right (⌥⌘])")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_split_pane_to_side(0, layout::Side::Right, cx);
                                })),
                        )
                    })
                    .when(is_horizontal && p_ix == 1, |d| {
                        d.child(
                            zbutton(("split-dock-top", p_ix))
                                .icon(IconName::ArrowUp)
                                .ghost()
                                .xsmall()
                                .tooltip("Dock pane to top (⌥⇧⌘[)")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_split_pane_to_side(1, layout::Side::Top, cx);
                                })),
                        )
                    })
                    .when(is_horizontal && p_ix == 0, |d| {
                        d.child(
                            zbutton(("split-dock-bottom", p_ix))
                                .icon(IconName::ArrowDown)
                                .ghost()
                                .xsmall()
                                .tooltip("Dock pane to bottom (⌥⇧⌘])")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_split_pane_to_side(0, layout::Side::Bottom, cx);
                                })),
                        )
                    })
                    .child(
                        zbutton(("split-swap", p_ix))
                            .icon(IconName::ArrowLeftRight)
                            .ghost()
                            .xsmall()
                            .tooltip("Swap pane positions")
                            .on_click(cx.listener(|this, _, _, cx| this.swap_pane_positions(cx))),
                    )
                    .child(
                        zbutton(("split-dir", p_ix))
                            .icon(dir_icon)
                            .ghost()
                            .xsmall()
                            .tooltip(crate::ui::format_keys(dir_tooltip))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.toggle_split_direction(cx)),
                            ),
                    )
                    .child(
                        zbutton(("split-close-pane", p_ix))
                            .icon(IconName::X)
                            .ghost()
                            .xsmall()
                            .tooltip("Close pane")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_split_pane(is_second_pane, window, cx);
                            })),
                    ),
            );

        let content = self.render_tab_view(tab, cx);
        let accent = self.theme.accent;
        let zone = move |a: f32| Rgba { a, ..accent };

        v_flex()
            .id(("split-pane-container", p_ix))
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .drag_over::<layout::DraggedSplitPane>(move |style, _, _, _| style.bg(zone(0.08)))
            .on_drop(cx.listener(
                move |this,
                      &layout::DraggedSplitPane(from_ix): &layout::DraggedSplitPane,
                      _,
                      cx| {
                    this.active_drag_split_pane = None;
                    if from_ix != p_ix {
                        this.swap_pane_positions(cx);
                    }
                },
            ))
            .drag_over::<layout::DraggedTab>(move |style, _, _, _| style.bg(zone(0.08)))
            .on_drop(cx.listener(
                move |this, &layout::DraggedTab(from_idx): &layout::DraggedTab, _, cx| {
                    this.active_drag_tab = None;
                    this.drop_tab_on_split_pane(from_idx, p_ix, cx);
                },
            ))
            .child(header)
            .child(
                div()
                    .flex_1()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .when(!is_active, |d| {
                        d.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.swap_split(cx)),
                        )
                    })
                    .child(content),
            )
            .into_any_element()
    }

    pub(crate) fn render_center(
        &mut self,
        active_tab: Option<&Tab>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme.clone();
        match active_tab {
            None if self.state.session.is_none() => self.render_onboarding(cx),
            None => self.render_launcher(cx),
            Some(tab) => match self.split_tab().filter(|_| self.state.tabs.len() > 1) {
                // Split view: Zed-style two pane layout with sleek headers, focus cues, and orientation support
                Some(split) => {
                    let (first, second) = if self.split_active_right {
                        (
                            self.render_split_pane(&split, false, false, cx),
                            self.render_split_pane(tab, true, true, cx),
                        )
                    } else {
                        (
                            self.render_split_pane(tab, true, false, cx),
                            self.render_split_pane(&split, false, true, cx),
                        )
                    };

                    match layout::split_direction() {
                        layout::SplitDirection::Vertical => {
                            let mut group = h_resizable("split-view:vertical")
                                .on_resize(layout::remember("split-view:vertical"));
                            if let Some(s0) = layout::get_size("split-view:vertical", 0) {
                                group = group.child(resizable_panel().size(s0).child(first));
                            } else {
                                group = group.child(resizable_panel().child(first));
                            }
                            if let Some(s1) = layout::get_size("split-view:vertical", 1) {
                                group = group.child(
                                    resizable_panel().size(s1).child(
                                        div()
                                            .size_full()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .border_l_1()
                                            .border_color(t.border)
                                            .child(second),
                                    ),
                                );
                            } else {
                                group = group.child(
                                    resizable_panel().child(
                                        div()
                                            .size_full()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .border_l_1()
                                            .border_color(t.border)
                                            .child(second),
                                    ),
                                );
                            }
                            group.into_any_element()
                        }
                        layout::SplitDirection::Horizontal => {
                            let mut group = v_resizable("split-view:horizontal")
                                .on_resize(layout::remember("split-view:horizontal"));
                            if let Some(s0) = layout::get_size("split-view:horizontal", 0) {
                                group = group.child(resizable_panel().size(s0).child(first));
                            } else {
                                group = group.child(resizable_panel().child(first));
                            }
                            if let Some(s1) = layout::get_size("split-view:horizontal", 1) {
                                group = group.child(
                                    resizable_panel().size(s1).child(
                                        div()
                                            .size_full()
                                            .min_h_0()
                                            .overflow_hidden()
                                            .border_t_1()
                                            .border_color(t.border)
                                            .child(second),
                                    ),
                                );
                            } else {
                                group = group.child(
                                    resizable_panel().child(
                                        div()
                                            .size_full()
                                            .min_h_0()
                                            .overflow_hidden()
                                            .border_t_1()
                                            .border_color(t.border)
                                            .child(second),
                                    ),
                                );
                            }
                            group.into_any_element()
                        }
                    }
                }
                None => self.render_tab_view(tab, cx),
            },
        }
    }
}
