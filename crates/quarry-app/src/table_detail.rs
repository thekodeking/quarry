// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! A table tab is the table's home: its data, its structure, and its DDL (OOUX)

use crate::runtime::run_db;
use crate::state::Tab;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::component::button::ButtonVariants;
use gpui::component::input::EditorState;
use gpui::component::{ActiveTheme, Sizable};
use gpui::*;

#[derive(Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum TableView {
    #[default]
    Data,
    Structure,
    Ddl,
    /// The table and its foreign-key neighbours (catalog only, nothing to load)
    Diagram,
}

/// Loaded the first time Structure or DDL is opened
#[derive(Clone)]
pub struct TableDetail {
    /// (name, unique, columns)
    pub indexes: Vec<(String, bool, String)>,
    pub ddl: Entity<EditorState>,
    /// See `quarry_core::schema::TableInfo`
    pub comments: Vec<(String, String)>,
    pub triggers: Vec<(String, String)>,
    pub stats: Vec<(String, String)>,
}

impl QuarryWindow {
    pub fn set_table_view(&mut self, view: TableView, window: &mut Window, cx: &mut Context<Self>) {
        self.schedule_session_save(cx);
        let ix = self.state.active_tab_index;
        let Some(Tab::Table {
            view: v,
            detail,
            schema_name,
            table_name,
            grid,
            ..
        }) = self.state.tabs.get_mut(ix)
        else {
            return;
        };
        *v = view;
        cx.notify();
        if view == TableView::Diagram {
            // Each diagram opens unzoomed, centred on the table
            self.diagram_zoom = 1.0;
            self.diagram_pan = None;
        }
        if matches!(view, TableView::Data | TableView::Diagram) || detail.is_some() {
            return;
        }
        let Some(session) = grid
            .read(cx)
            .delegate()
            .source
            .as_ref()
            .map(|s| s.session.clone())
        else {
            return;
        };
        let (schema, table) = (schema_name.clone(), table_name.clone());
        let Some(tbl) = self
            .state
            .catalog
            .find_table(Some(&schema), &table)
            .cloned()
        else {
            return;
        };
        let fut = async move { session.table_info(&tbl).await.map_err(|e| e.to_string()) };
        run_db(window, cx, fut, move |this, res, window, cx| {
            let detail = res.map(|info| {
                let ddl = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language("sql")
                        .line_number(true)
                        .folding(true)
                        .default_value(info.ddl)
                });
                let indexes = info
                    .indexes
                    .into_iter()
                    .map(|i| (i.name, i.unique, i.columns))
                    .collect();
                Box::new(TableDetail {
                    indexes,
                    ddl,
                    comments: info.comments,
                    triggers: info.triggers,
                    stats: info.stats,
                })
            });
            // The tab may have moved or closed while loading
            if let Some(Tab::Table { detail: d, .. }) = this.state.tabs.iter_mut().find(|t| matches!(t, Tab::Table { schema_name: s, table_name: n, .. } if *s == schema && *n == table)) {
                *d = Some(detail);
            }
        });
    }

    /// Data · Structure · DDL
    pub fn render_table_view_switch(
        &self,
        current: TableView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        use gpui::assets::IconName;
        let views = [
            (TableView::Data, "Data", IconName::Table),
            (TableView::Structure, "Structure", IconName::TableProperties),
            (TableView::Ddl, "DDL", IconName::FileCode),
            (TableView::Diagram, "Diagram", IconName::Network),
        ];
        h_flex()
            .items_center()
            .p_0p5()
            .gap_0p5()
            .rounded(px(crate::ui::RADIUS_MD))
            .bg(t.bg_panel)
            .border_1()
            .border_color(t.border)
            .children(views.map(|(view, label, icon)| {
                let on = view == current;
                h_flex()
                    .id(label)
                    .h(px(crate::ui::SMALL_H - 2.0))
                    .gap_1p5()
                    .items_center()
                    .px_2()
                    .rounded(px(crate::ui::RADIUS_SM))
                    .text_xs()
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(t.bg_selected)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                    })
                    .when(!on, |d| {
                        d.text_color(t.text_muted)
                            .hover(|s| s.bg(t.bg_hover).text_color(t.text_secondary))
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.set_table_view(view, window, cx)
                        }),
                    )
                    .child(gpui::component::Icon::new(icon).xsmall().when(on, |i| i.text_color(t.accent)))
                    .child(label)
            }))
    }

    pub fn render_table_detail(&self, tab: &Tab, cx: &mut Context<Self>) -> AnyElement {
        let Tab::Table {
            view,
            detail,
            schema_name,
            table_name,
            ..
        } = tab
        else {
            return div().into_any_element();
        };
        let t = &self.theme;
        if *view == TableView::Diagram {
            return self.render_diagram(schema_name, table_name, cx);
        }
        let detail = match detail {
            None => {
                return div()
                    .p_4()
                    .text_sm()
                    .text_color(t.text_muted)
                    .child("Loading…")
                    .into_any_element()
            }
            Some(Err(e)) => {
                return div()
                    .p_4()
                    .text_sm()
                    .text_color(t.danger)
                    .child(e.clone())
                    .into_any_element()
            }
            Some(Ok(d)) => d,
        };
        if *view == TableView::Ddl {
            return v_flex()
                .size_full()
                .child(
                    h_flex().items_center().px_3().py_1().justify_end().child(
                        zbutton("ddl-to-query")
                            .pad_xs()
                            .label("Open in query tab")
                            .ghost()
                            .small()
                            .on_click({
                                let ddl = detail.ddl.clone();
                                cx.listener(move |this, _, window, cx| {
                                    let sql = ddl.read(cx).value().to_string();
                                    this.open_query_tab(&sql, window, cx);
                                })
                            }),
                    ),
                )
                .child(
                    div().flex_1().min_h_0().child(
                        gpui::component::input::Editor::new(&detail.ddl)
                            .h_full()
                            .bordered(false),
                    ),
                )
                .into_any_element();
        }

        let Some(table) = self.state.catalog.find_table(Some(schema_name), table_name) else {
            return div()
                .p_4()
                .text_sm()
                .text_color(t.text_muted)
                .child("Not in the schema; refresh it from the connection's menu")
                .into_any_element();
        };
        let mono = cx.theme().mono_font_family.clone();
        let heading = |s: &'static str, count: Option<usize>| {
            h_flex()
                .items_center()
                .pt_5()
                .pb_1()
                .gap_2()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_muted)
                .child(s)
                .children(count.map(|n| {
                    div()
                        .px_1p5()
                        .rounded(px(crate::ui::RADIUS_SM))
                        .bg(t.bg_surface)
                        .font_weight(FontWeight::NORMAL)
                        .child(n.to_string())
                }))
        };
        let none = |s: &'static str| div().py_1().text_sm().text_color(t.text_muted).child(s);
        let cell = |w: f32, text: String, color: Rgba| {
            div()
                .w(px(w))
                .flex_shrink_0()
                .truncate()
                .text_color(color)
                .child(text)
        };
        let rest = |text: String, color: Rgba| {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(color)
                .child(text)
        };
        let row = || {
            h_flex()
                .items_center()
                .py_1()
                .px_2()
                .gap_3()
                .border_b_1()
                .border_color(t.border)
                .text_sm()
                .font_family(mono.clone())
                .hover(|s| s.bg(t.bg_hover))
        };
        let head = || {
            h_flex()
                .items_center()
                .py_1()
                .px_2()
                .gap_3()
                .border_b_1()
                .border_color(t.border)
                .bg(t.bg_panel)
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
        };
        let badge = |label: &'static str, color: Rgba| {
            div()
                .px_1()
                .rounded(px(crate::ui::RADIUS_SM))
                .bg(crate::Theme::tint(color, 0.15))
                .text_color(color)
                .child(label)
        };
        let open_table = |schema: String, table: String| {
            cx.listener(move |this, _, window, cx| {
                this.open_pin(
                    crate::pins::Pin::Table {
                        schema: schema.clone(),
                        table: table.clone(),
                    },
                    window,
                    cx,
                )
            })
        };
        let comment_of = |name: &str| {
            detail
                .comments
                .iter()
                .find(|(c, _)| c == name)
                .map(|(_, v)| v.clone())
        };
        // Single-column unique indexes make that column UNIQUE
        let unique_cols: Vec<&str> = detail
            .indexes
            .iter()
            .filter(|(_, unique, cols)| *unique && !cols.contains(','))
            .map(|(_, _, cols)| cols.trim())
            .collect();
        let fk_target = |f: &quarry_core::schema::ForeignKeyRef| {
            let schema = if f.to_schema.is_empty() {
                table.schema_name.clone()
            } else {
                f.to_schema.clone()
            };
            (schema, f.to_table.clone())
        };

        let referenced_by: Vec<_> = self
            .state
            .catalog
            .tables
            .iter()
            .flat_map(|o| {
                o.foreign_keys
                    .iter()
                    .filter(|f| f.to_table == table.table_name)
                    .map(move |f| {
                        (
                            (o.schema_name.clone(), o.table_name.clone()),
                            format!("{}.{}", o.table_name, f.from_column),
                            f.to_column.clone(),
                        )
                    })
            })
            .collect();

        // ── Summary: what this table is at a glance ──
        let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
        let summary = v_flex()
            .pt_4()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .items_baseline()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text_primary)
                            .child(format!("{}.{}", table.schema_name, table.table_name)),
                    )
                    .when(table.is_view, |d| d.child(badge("VIEW", t.accent)))
                    .child(div().text_sm().text_color(t.text_muted).child(format!(
                        "{} · {} · {} · referenced by {}",
                        count(table.columns.len(), "column"),
                        count(detail.indexes.len(), "index"),
                        count(table.foreign_keys.len(), "foreign key"),
                        referenced_by.len()
                    ))),
            )
            .children(comment_of("").map(|c| div().text_sm().text_color(t.text_secondary).child(c)))
            .when(!detail.stats.is_empty(), |d| {
                d.child(h_flex().items_center().flex_wrap().gap_2().children(
                    detail.stats.iter().map(|(label, value)| {
                        v_flex()
                            .px_2p5()
                            .py_1p5()
                            .rounded(px(crate::ui::RADIUS_MD))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.bg_panel)
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(t.text_muted)
                                    .child(label.clone()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family(mono.clone())
                                    .text_color(t.text_primary)
                                    .child(value.clone()),
                            )
                    }),
                ))
            });

        // ── Columns ──
        let columns = v_flex()
            .child(
                head()
                    .text_color(t.text_muted)
                    .child(cell(24.0, "#".into(), t.text_muted))
                    .child(cell(180.0, "Name".into(), t.text_muted))
                    .child(cell(150.0, "Type".into(), t.text_muted))
                    .child(cell(50.0, "Null".into(), t.text_muted))
                    .child(cell(150.0, "Default".into(), t.text_muted))
                    .child(cell(220.0, "Keys".into(), t.text_muted))
                    .child(rest("Comment".into(), t.text_muted)),
            )
            .children(table.columns.iter().enumerate().map(|(ix, c)| {
                let fk = table.foreign_keys.iter().find(|f| f.from_column == c.name);
                let unique = !c.is_primary_key && unique_cols.contains(&c.name.as_str());
                let keys = h_flex()
                    .items_center()
                    .w(px(220.0))
                    .flex_shrink_0()
                    .gap_1()
                    .when(c.is_primary_key, |d| d.child(badge("PK", t.warning)))
                    .when(unique, |d| d.child(badge("UQ", t.success)))
                    .when_some(fk, |d, f| {
                        let (schema, to) = fk_target(f);
                        d.child(badge("FK", t.accent)).child(
                            div()
                                .id(("fk", ix))
                                .truncate()
                                .text_color(t.accent)
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .on_click(open_table(schema, to))
                                .child(format!("{}.{}", f.to_table, f.to_column)),
                        )
                    });
                row()
                    .child(cell(24.0, (ix + 1).to_string(), t.text_muted))
                    .child(
                        cell(180.0, c.name.clone(), t.text_primary)
                            .when(c.is_primary_key, |d| d.font_weight(FontWeight::SEMIBOLD)),
                    )
                    .child(cell(
                        150.0,
                        c.raw_type_name.to_lowercase(),
                        t.text_secondary,
                    ))
                    .child(cell(
                        50.0,
                        if c.is_nullable { "yes" } else { "no" }.into(),
                        if c.is_nullable {
                            t.text_muted
                        } else {
                            t.text_secondary
                        },
                    ))
                    .child(cell(
                        150.0,
                        c.default_value.clone().unwrap_or_default(),
                        t.text_muted,
                    ))
                    .child(keys)
                    .child(rest(comment_of(&c.name).unwrap_or_default(), t.text_muted))
            }));

        // ── Indexes ──
        let indexes = v_flex()
            .when(detail.indexes.is_empty(), |d| d.child(none("None")))
            .when(!detail.indexes.is_empty(), |d| {
                d.child(
                    head()
                        .text_color(t.text_muted)
                        .child(cell(240.0, "Name".into(), t.text_muted))
                        .child(cell(70.0, "Kind".into(), t.text_muted))
                        .child(rest("Columns".into(), t.text_muted)),
                )
            })
            .children(detail.indexes.iter().map(|(name, unique, cols)| {
                let kind = h_flex()
                    .items_center()
                    .w(px(70.0))
                    .flex_shrink_0()
                    .when(*unique, |d| d.child(badge("UNIQUE", t.success)));
                row()
                    .child(cell(240.0, name.clone(), t.text_primary))
                    .child(kind)
                    .child(rest(cols.clone(), t.text_secondary))
            }));

        // ── Relationships, both ways ──
        let link_row =
            |id: (&'static str, usize), from: String, to: String, target: (String, String)| {
                row()
                    .child(cell(240.0, from, t.text_primary))
                    .child(div().text_color(t.text_muted).child("→"))
                    .child(
                        div()
                            .id(id)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(t.accent)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(open_table(target.0, target.1))
                            .child(to),
                    )
            };
        let foreign_keys = v_flex()
            .when(table.foreign_keys.is_empty(), |d| d.child(none("None")))
            .children(table.foreign_keys.iter().enumerate().map(|(i, f)| {
                link_row(
                    ("fk-out", i),
                    f.from_column.clone(),
                    format!("{}.{}", f.to_table, f.to_column),
                    fk_target(f),
                )
            }));
        let ref_count = referenced_by.len();
        let referenced_by = v_flex()
            .when(ref_count == 0, |d| {
                d.child(none("No other table points here"))
            })
            .children(
                referenced_by
                    .into_iter()
                    .enumerate()
                    .map(|(i, (target, from, to))| {
                        link_row(
                            ("ref-by", i),
                            from,
                            format!("{}.{}", table.table_name, to),
                            target,
                        )
                    }),
            );

        // ── Triggers ──
        let triggers = v_flex()
            .when(detail.triggers.is_empty(), |d| d.child(none("None")))
            .children(detail.triggers.iter().map(|(name, def)| {
                row()
                    .items_start()
                    .child(cell(240.0, name.clone(), t.text_primary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(t.text_secondary)
                            .child(def.clone()),
                    )
            }));

        let section_card = |content: Div| {
            content
                .rounded(px(crate::ui::RADIUS_MD))
                .border_1()
                .border_color(t.border)
                .overflow_hidden()
        };

        div()
            .id("table-structure")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .pb_6()
            .child(summary)
            .child(heading("COLUMNS", Some(table.columns.len())))
            .child(section_card(columns))
            .child(heading("INDEXES", Some(detail.indexes.len())))
            .child(if detail.indexes.is_empty() {
                indexes
            } else {
                section_card(indexes)
            })
            .child(heading("FOREIGN KEYS", Some(table.foreign_keys.len())))
            .child(if table.foreign_keys.is_empty() {
                foreign_keys
            } else {
                section_card(foreign_keys)
            })
            .child(heading("REFERENCED BY", Some(ref_count)))
            .child(if ref_count == 0 {
                referenced_by
            } else {
                section_card(referenced_by)
            })
            .child(heading("TRIGGERS", Some(detail.triggers.len())))
            .child(if detail.triggers.is_empty() {
                triggers
            } else {
                section_card(triggers)
            })
            .into_any_element()
    }
}
