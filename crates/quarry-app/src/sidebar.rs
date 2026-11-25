// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Database explorer sidebar tree, connection switching, and schema object navigation.
//!
//! Renders the hierarchical tree of saved connections, database schemas, tables, views,
//! functions, procedures, sequences, and triggers.

use crate::object_menu::DbObject;
use crate::state::{AppState, Environment, Tab};
use crate::theme::Theme;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::{Icon, Sizable};
use gpui::*;

/// DataGrip-style database explorer: connections → schemas → tables/views.
pub struct Sidebar;

impl Sidebar {
    pub fn render(
        state: &AppState,
        theme: &Theme,
        dock: crate::layout::Dock,
        cx: &Context<QuarryWindow>,
    ) -> impl IntoElement {
        if !state.sidebar_visible {
            return div().into_any_element();
        }

        let active_id = state.active_connection.as_ref().map(|c| c.id.clone());
        let active_table = state.active_tab().and_then(|t| match t {
            crate::state::Tab::Table {
                table_name,
                schema_name,
                ..
            } => Some((schema_name.as_str(), table_name.as_str())),
            _ => None,
        });

        let icon = |name: IconName, color: Rgba| Icon::new(name).xsmall().text_color(color);
        let row = |id: ElementId, depth: f32| {
            let mut d = h_flex()
                .id(id)
                .relative()
                .h(px(26.0))
                .pl(px(8.0 + depth * 16.0))
                .pr_2()
                .gap_1p5()
                .items_center()
                .rounded(px(crate::ui::RADIUS_SM))
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover));
            for level in 0..(depth as usize) {
                d = d.child(
                    div()
                        .absolute()
                        .left(px(8.0 + (level as f32) * 16.0 + 5.0))
                        .top_0()
                        .bottom_0()
                        .w(px(1.0))
                        .bg(theme.border.opacity(0.75)),
                );
            }
            d
        };
        // Leaves skip the chevron column, so a spacer keeps their icon under the parent's label
        let leaf =
            |id: ElementId, depth: f32| row(id, depth).child(div().w(px(12.0)).flex_shrink_0());
        let menu_on = |obj: DbObject| {
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.open_sidebar_menu(obj.clone(), window, cx)
            })
        };
        let label = |text: String, color: Rgba| {
            div()
                .flex_1()
                .text_sm()
                .text_color(color)
                .truncate()
                .child(text)
        };
        let muted = |text: String| div().text_sm().text_color(theme.text_muted).child(text);

        let mut tree = v_flex().py_1p5().px_2();
        if state.saved_connections.is_empty() {
            tree = tree.child(
                v_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child("No connections yet."),
                    )
                    .child(
                        zbutton("sidebar-empty-add")
                            .pad_sm()
                            .label("Add connection")
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_add_conn(window, cx)
                            })),
                    ),
            );
        }

        // Grouped by environment, so prod connections are never mixed in with local ones
        let mut conns: Vec<_> = state.saved_connections.iter().enumerate().collect();
        conns.sort_by_key(|(_, c)| c.env as u8);
        let mut last_env = None;
        for (conn_ix, conn) in conns {
            if last_env != Some(conn.env) {
                last_env = Some(conn.env);
                let color = match conn.env {
                    Environment::Local => theme.success,
                    Environment::Staging => theme.warning,
                    Environment::Prod => theme.danger,
                };
                tree = tree.child(
                    h_flex()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .gap_1p5()
                        .items_center()
                        .child(div().size(px(6.0)).rounded_full().bg(color))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_muted)
                                .child(conn.env.name()),
                        ),
                );
            }
            let is_active = active_id.as_deref() == Some(conn.id.as_str());
            let expanded = is_active && !state.collapsed_connections.contains(&conn.id);
            let kind = conn.driver.as_str();
            let conn_id = conn.id.clone();
            tree = tree.child(
                row(("conn", conn_ix).into(), 0.0)
                    .on_mouse_down(
                        MouseButton::Right,
                        menu_on(DbObject::Connection {
                            id: conn.id.clone(),
                            name: conn.name.clone(),
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.connect_saved(conn_id.clone(), window, cx)
                    }))
                    .child(icon(
                        if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        theme.text_muted,
                    ))
                    .child(crate::db_icons::database_icon(
                        kind,
                        if is_active {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    ))
                    .child(label(
                        conn.name.clone(),
                        if is_active {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        },
                    )),
            );
            if !expanded {
                continue;
            }

            // Pinned items first (U6): this connection's favourite tables and saved queries
            let pinned = state.pins.get(&conn.id).cloned().unwrap_or_default();
            for (pin_ix, pin) in pinned.into_iter().enumerate() {
                let (open_pin, drop_pin) = (pin.clone(), pin.clone());
                let is_table = matches!(pin, crate::pins::Pin::Table { .. });
                tree = tree.child(
                    leaf(("pin", pin_ix).into(), 1.0)
                        .group("pin-row")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_pin(open_pin.clone(), window, cx)
                        }))
                        .child(icon(IconName::Pin, theme.accent))
                        .child(icon(
                            if is_table {
                                IconName::Table
                            } else {
                                IconName::SquareTerminal
                            },
                            theme.text_muted,
                        ))
                        .child(label(pin.label().to_string(), theme.text_primary))
                        .child(
                            div()
                                .id(("unpin", pin_ix))
                                .invisible()
                                .group_hover("pin-row", |s| s.visible())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_pin(drop_pin.clone(), cx)
                                }))
                                .child(icon(IconName::PinOff, theme.text_muted)),
                        ),
                );
            }

            for (schema_ix, schema) in state.catalog.schemas.iter().enumerate() {
                let tables: Vec<_> = state
                    .catalog
                    .tables
                    .iter()
                    .filter(|t| &t.schema_name == schema)
                    .collect();
                let expanded = state.expanded_schemas.contains(schema);
                let schema_name = schema.clone();
                tree = tree.child(
                    row(("schema", schema_ix).into(), 1.0)
                        .on_mouse_down(
                            MouseButton::Right,
                            menu_on(DbObject::Schema(schema.clone())),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_sidebar_schema(schema_name.clone(), cx)
                        }))
                        .child(icon(
                            if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            },
                            theme.text_muted,
                        ))
                        .child(icon(IconName::Layers, theme.text_muted))
                        .child(label(schema.clone(), theme.text_secondary))
                        // Not loaded yet (big databases): no count rather than a wrong 0
                        .when(state.catalog.is_loaded(schema), |d| {
                            d.child(
                                div()
                                    .px_1p5()
                                    .rounded(px(crate::ui::RADIUS_SM))
                                    .bg(theme.bg_surface)
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(tables.len().to_string()),
                            )
                        }),
                );
                if !expanded {
                    continue;
                }
                if !state.catalog.is_loaded(schema) {
                    tree = tree.child(
                        leaf(("schema-loading", schema_ix).into(), 2.0)
                            .child(muted("Loading…".to_string())),
                    );
                    continue;
                }
                for (tbl_ix, tbl) in tables.into_iter().enumerate() {
                    let selected =
                        active_table == Some((tbl.schema_name.as_str(), tbl.table_name.as_str()));
                    let (t_name, s_name) = (tbl.table_name.clone(), tbl.schema_name.clone());
                    let (h_name, h_schema) = (t_name.clone(), s_name.clone());
                    tree = tree.child(
                        leaf(("table", schema_ix * 100_000 + tbl_ix).into(), 2.0)
                            .when(selected, |r| r.bg(theme.bg_selected))
                            .on_mouse_down(
                                MouseButton::Right,
                                menu_on(DbObject::Table {
                                    schema: tbl.schema_name.clone(),
                                    table: tbl.table_name.clone(),
                                }),
                            )
                            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                let keep = e.click_count() >= 2;
                                this.open_sidebar_table(&t_name, &s_name, keep, window, cx)
                            }))
                            // Peek (U7): hovering previews the table without opening it
                            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                this.peek_hover(
                                    h_schema.clone(),
                                    h_name.clone(),
                                    *hovered,
                                    window,
                                    cx,
                                )
                            }))
                            .child(icon(
                                if tbl.is_view {
                                    IconName::Eye
                                } else {
                                    IconName::Table
                                },
                                if selected {
                                    theme.accent
                                } else {
                                    theme.text_muted
                                },
                            ))
                            .child(label(
                                tbl.table_name.clone(),
                                if selected {
                                    theme.text_primary
                                } else {
                                    theme.text_secondary
                                },
                            )),
                    );
                }

                // Functions, sequences, triggers, types: one collapsed group per kind, loaded on expand
                let objects = state.schema_objects.get(schema);
                if matches!(objects, Some(None)) {
                    tree = tree.child(
                        leaf(("objects-loading", schema_ix).into(), 2.0)
                            .child(muted("Loading…".into())),
                    );
                }
                let objects = objects.and_then(|o| o.as_deref()).unwrap_or_default();
                for (kind_ix, kind) in quarry_core::schema::ObjectKind::ALL.into_iter().enumerate()
                {
                    let of_kind: Vec<_> = objects.iter().filter(|o| o.kind == kind).collect();
                    if of_kind.is_empty() {
                        continue;
                    }
                    let key = format!("{schema}/{}", kind.plural());
                    let open = state.expanded_object_groups.contains(&key);
                    let group_id = schema_ix * 10 + kind_ix;
                    tree = tree.child(
                        row(("object-group", group_id).into(), 2.0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_sidebar_group(key.clone(), cx)
                            }))
                            .child(icon(
                                if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                theme.text_muted,
                            ))
                            .child(icon(object_icon(kind), theme.text_muted))
                            .child(label(kind.plural().to_string(), theme.text_secondary))
                            .child(muted(of_kind.len().to_string())),
                    );
                    if !open {
                        continue;
                    }
                    for (obj_ix, obj) in of_kind.into_iter().enumerate() {
                        let (s_name, o) = (schema.clone(), obj.clone());
                        tree = tree.child(
                            leaf(("object", group_id * 100_000 + obj_ix).into(), 3.0)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_object_definition(
                                        s_name.clone(),
                                        o.clone(),
                                        window,
                                        cx,
                                    )
                                }))
                                .child(icon(object_icon(kind), theme.text_muted))
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w(px(140.0))
                                        .text_sm()
                                        .text_color(theme.text_secondary)
                                        .truncate()
                                        .child(obj.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .text_color(theme.text_muted)
                                        .truncate()
                                        .child(obj.detail.clone()),
                                ),
                        );
                    }
                }
            }
        }

        let active_file = state.active_tab().and_then(|t| match t {
            crate::state::Tab::Query { file, .. } => file.clone(),
            _ => None,
        });
        let queries = crate::files::saved_queries();
        let recent = crate::files::recent_files();
        let mut query_list = v_flex().py_1().px_2().gap_px();
        if queries.is_empty() && recent.is_empty() {
            query_list = query_list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(crate::ui::format_keys("Save a query (⌘S) to see it here.")),
            );
        }
        let rename_row = |input: &Entity<gpui::component::input::InputState>| {
            h_flex()
                .h(px(ROW_H))
                .pl_2()
                .pr_1()
                .gap_1p5()
                .items_center()
                .child(icon(IconName::FileCode, theme.accent))
                .child(
                    div()
                        .flex_1()
                        .child(gpui::component::input::Input::new(input).xsmall()),
                )
        };
        for (ix, path) in queries.into_iter().enumerate() {
            if let Some((_, input)) = state.renaming_query.as_ref().filter(|(p, _)| *p == path) {
                query_list = query_list.child(rename_row(input));
                continue;
            }
            let selected = active_file.as_ref() == Some(&path);
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            query_list = query_list.child(
                row(("query-file", ix).into(), 0.0)
                    .when(selected, |r| r.bg(theme.bg_selected))
                    .on_mouse_down(MouseButton::Right, menu_on(DbObject::Query(path.clone())))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_query_file(path.clone(), window, cx)
                    }))
                    .child(icon(
                        IconName::FileCode,
                        if selected {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    ))
                    .child(label(
                        name,
                        if selected {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        },
                    )),
            );
        }
        if !recent.is_empty() {
            query_list = query_list.child(
                div()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child("Recent"),
            );
        }
        for (ix, path) in recent.into_iter().enumerate() {
            if let Some((_, input)) = state.renaming_query.as_ref().filter(|(p, _)| *p == path) {
                query_list = query_list.child(rename_row(input));
                continue;
            }
            let selected = active_file.as_ref() == Some(&path);
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let folder = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            query_list = query_list.child(
                row(("recent-file", ix).into(), 0.0)
                    .when(selected, |r| r.bg(theme.bg_selected))
                    .on_mouse_down(MouseButton::Right, menu_on(DbObject::Query(path.clone())))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_query_file(path.clone(), window, cx)
                    }))
                    .child(icon(
                        IconName::FileCode,
                        if selected {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    ))
                    .child(label(
                        name,
                        if selected {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        },
                    ))
                    .child(muted(folder)),
            );
        }
        let section_title = |text: &'static str| {
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(text)
        };

        let is_queries = state.sidebar_tab == crate::state::SidebarTab::Queries;
        let is_right = dock.sidebar == crate::layout::Side::Right;
        let this_entity = cx.entity().clone();
        let header_actions = if is_queries {
            h_flex()
                .items_center()
                .gap_0p5()
                .child(
                    zbutton("sidebar-new-query")
                        .icon(IconName::Plus)
                        .ghost()
                        .xsmall()
                        .tooltip(format_keys("New query (⌘N)"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.new_query(&crate::NewQuery, window, cx)
                        })),
                )
                .child(
                    zbutton("sidebar-open-file")
                        .icon(IconName::FolderOpen)
                        .ghost()
                        .xsmall()
                        .tooltip(format_keys("Open file (⌘O)"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_sql_file(window, cx)
                        })),
                )
        } else {
            h_flex()
                .items_center()
                .gap_0p5()
                .child(
                    zbutton("sidebar-add-conn")
                        .icon(IconName::Plus)
                        .ghost()
                        .xsmall()
                        .tooltip(format_keys("New connection (⇧⌘N)"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_add_conn(window, cx)
                        })),
                )
                .when(state.session.is_some(), |d| {
                    d.child(
                        zbutton("sidebar-refresh-schema")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .tooltip("Refresh schema")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh_catalog(window, cx)
                            })),
                    )
                })
                .child(
                    zbutton("sidebar-collapse-all")
                        .icon(IconName::Minus)
                        .ghost()
                        .xsmall()
                        .tooltip("Collapse all")
                        .on_click(cx.listener(|this, _, _, cx| this.collapse_sidebar_all(cx))),
                )
        };

        v_flex()
            .size_full()
            .bg(theme.bg_sidebar)
            .when(is_right, |d| d.border_l_1())
            .when(!is_right, |d| d.border_r_1())
            .border_color(theme.border)
            .child(
                h_flex()
                    .id("sidebar-header")
                    .h(px(BAR_H))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .cursor_grab()
                    .on_drag(crate::layout::DraggedPane(crate::layout::Pane::Sidebar), {
                        let this_entity = this_entity.clone();
                        move |_, _, _, cx| {
                            this_entity.update(cx, |this, _| {
                                this.active_drag_pane = Some(crate::layout::Pane::Sidebar);
                            });
                            cx.new(|_| DragGhost::new("Database Explorer"))
                        }
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _, window, cx| {
                            this.open_sidebar_dock_menu(window, cx);
                        }),
                    )
                    .child(section_title(if is_queries {
                        "Saved Queries"
                    } else {
                        "Database Explorer"
                    }))
                    .child(header_actions),
            )
            .child(if is_queries {
                div()
                    .id("sidebar-queries-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(query_list)
                    .into_any_element()
            } else {
                div()
                    .id("sidebar-tree-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(tree)
                    .into_any_element()
            })
            .into_any_element()
    }
}

fn object_icon(kind: quarry_core::schema::ObjectKind) -> IconName {
    use quarry_core::schema::ObjectKind::*;
    match kind {
        Function => IconName::SquareFunction,
        Procedure => IconName::Workflow,
        Sequence => IconName::ListOrdered,
        Trigger => IconName::Zap,
        Type => IconName::Tag,
    }
}

/// Sidebar clicks that do more than call one existing method
impl QuarryWindow {
    fn open_sidebar_table(
        &mut self,
        table: &str,
        schema: &str,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.peek = None;
        self.peek_task = None;
        // One click previews in a tab the next click reuses; a double-click keeps it
        let opening = self.state.find_tab(table).is_none();
        let replace = (opening && !keep)
            .then(|| self.preview_tab.as_deref())
            .flatten()
            .and_then(|id| self.state.tabs.iter().position(|t| t.id() == id))
            .filter(|&ix| self.tab_untouched(ix, cx));
        if let Some(ix) = replace {
            self.close_tab_at(ix, cx);
        }
        self.open_table_tab(table, schema, window, cx);
        let active = self.state.active_tab().map(|t| t.id().to_string());
        if let Some(ix) = replace {
            self.reorder_tab(self.state.tabs.len() - 1, ix, cx);
        }
        if opening && !keep {
            self.preview_tab = active;
        } else if keep && self.preview_tab == active {
            self.preview_tab = None;
        }
        self.refresh_active_tab(window, cx);
        cx.notify();
    }

    /// A table tab with no filter typed and no unsaved edits: safe to swap for another preview
    fn tab_untouched(&self, ix: usize, cx: &App) -> bool {
        match self.state.tabs.get(ix) {
            Some(tab @ Tab::Table { filter, .. }) => {
                filter.read(cx).value().is_empty()
                    && tab.grid().read(cx).delegate().pending.is_empty()
            }
            _ => false,
        }
    }

    fn toggle_sidebar_schema(&mut self, schema: String, cx: &mut Context<Self>) {
        self.state.toggle_schema(&schema);
        if self.state.expanded_schemas.contains(&schema) {
            self.load_schemas(vec![schema], cx);
        }
        cx.notify();
    }

    fn open_sidebar_menu(&mut self, obj: DbObject, window: &mut Window, cx: &mut Context<Self>) {
        self.peek = None;
        self.open_object_menu(obj, window, cx)
    }

    fn toggle_sidebar_group(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.state.expanded_object_groups.remove(&key) {
            self.state.expanded_object_groups.insert(key);
        }
        cx.notify();
    }

    fn collapse_sidebar_all(&mut self, cx: &mut Context<Self>) {
        self.state.expanded_schemas.clear();
        self.state.expanded_object_groups.clear();
        for conn in &self.state.saved_connections {
            self.state.collapsed_connections.insert(conn.id.clone());
        }
        cx.notify();
    }
}
