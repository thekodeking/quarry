// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! SQL query editor view, toolbar controls, gutter run buttons, and modal vim mode integration.
//!
//! Renders the primary text editor with schema switching, row limits, soft-wrapping toggles,
//! line numbering, execution controls, and Vim state indicators.

use crate::state::Environment;
use crate::theme::Theme;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{Editor, EditorState};
use gpui::component::{ActiveTheme, Icon, Sizable};
use gpui::*;

/// Renders the SQL query editor pane with interactive toolbars and execution controls.
pub struct QueryEditor;

const RUN_COL_W: f32 = 18.0;

impl QueryEditor {
    /// `connection`: the active connection's name and environment, shown at the left of the toolbar
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        editor: &Entity<EditorState>,
        is_running: bool,
        soft_wrap: bool,
        connection: Option<(&str, Environment)>,
        // `Some` when the dialect can switch schemas: the tab's schema label ("default" when unset)
        schema: Option<String>,
        row_limit: Option<u32>,
        // How the connection's dialect splits statements, for the ▶ buttons
        rules: quarry_sql::SplitRules,
        vim_mode: Option<crate::state::VimMode>,
        // Font size for a block cursor painted over the caret (Vim's Normal / Visual modes)
        block_cursor: Option<f32>,
        theme: &Theme,
        cx: &Context<QuarryWindow>,
    ) -> impl IntoElement {
        let picker = |id: &'static str, icon: IconName, text: String, on: bool| {
            h_flex()
                .id(id)
                .h(px(crate::ui::SMALL_H))
                .px_2()
                .gap_1()
                .items_center()
                .rounded(px(crate::ui::RADIUS_SM))
                .cursor_pointer()
                .text_sm()
                .text_color(if on {
                    theme.text_secondary
                } else {
                    theme.text_muted
                })
                .hover(|s| s.bg(theme.bg_hover))
                .child(Icon::new(icon).xsmall().text_color(theme.text_muted))
                .child(text)
                .child(
                    Icon::new(IconName::ChevronDown)
                        .xsmall()
                        .text_color(theme.text_muted),
                )
        };
        let schema_picker = schema.map(|s| {
            picker("btn-schema", IconName::Layers, s, true)
                .on_click(cx.listener(move |this, _, window, cx| this.open_schema_menu(window, cx)))
        });
        let limit_picker = picker(
            "btn-limit",
            IconName::ListFilter,
            row_limit.map_or("No limit".to_string(), |n| format!("Limit {n}")),
            row_limit.is_some(),
        )
        .on_click(cx.listener(move |this, _, window, cx| this.open_limit_menu(window, cx)));
        let is_prod = connection.is_some_and(|(_, env)| env == Environment::Prod);
        let conn_label = match connection {
            Some((name, env)) => h_flex()
                .gap_1p5()
                .items_center()
                .child(div().size(px(6.0)).rounded_full().bg(env_color(env, theme)))
                .child(
                    div()
                        .max_w(px(160.0))
                        .truncate()
                        .text_sm()
                        .text_color(theme.text_secondary)
                        .child(name.to_string()),
                )
                // The dot says local; staging and production also say so in words
                .when(env != Environment::Local, |d| {
                    d.child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(env_color(env, theme))
                            .child(env.name()),
                    )
                }),
            None => h_flex().child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child("Choose a connection"),
            ),
        };
        let conn_label = conn_label
            .id("btn-connection")
            // Never narrower than its contents (they'd paint over the schema picker); the name caps instead
            .flex_shrink_0()
            .gap_1p5()
            .items_center()
            .h(px(crate::ui::SMALL_H))
            .px_2()
            .rounded(px(crate::ui::RADIUS_SM))
            .cursor_pointer()
            .hover(|s| s.bg(theme.bg_hover))
            .on_click(cx.listener(move |this, _, window, cx| this.open_connection_menu(window, cx)))
            .child(
                Icon::new(IconName::ChevronDown)
                    .xsmall()
                    .text_color(theme.text_muted),
            );
        v_flex()
            .size_full()
            .min_w_0()
            .bg(theme.bg_panel)
            .child(
                // min_w_0: a flex item defaults to its content's width, so without it the bar never narrows
                h_flex()
                    .w_full()
                    .min_w_0()
                    .h(px(CONTROL_H))
                    .px_3()
                    // A faint red wash on prod, so it's obvious before you run anything
                    .bg(if is_prod {
                        Theme::tint(theme.danger, 0.08)
                    } else {
                        theme.bg_sidebar.into()
                    })
                    .border_b_1()
                    .border_color(theme.border)
                    .gap_1()
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .gap_1()
                            .items_center()
                            .child(conn_label)
                            .children(schema_picker.map(|p| p.flex_shrink_0()))
                            .child(limit_picker.flex_shrink_0())
                            .when(editor.read(cx).value().len() > 512 * 1024, |d| {
                                d.child(
                                    div()
                                        .px_1p5()
                                        .py(px(1.0))
                                        .rounded(px(crate::ui::RADIUS_SM))
                                        .bg(theme.bg_surface)
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child("Large file"),
                                )
                            })

                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_1()
                            .items_center()
                            .child(toolbar_button(
                                "btn-save",
                                IconName::Save,
                                "Save",
                                "Save (⌘S)",
                                Box::new(crate::SaveFile),
                            ))
                            .child(toolbar_button(
                                "btn-format",
                                IconName::Wand,
                                "Format",
                                "Format (⇧⌥F)",
                                Box::new(crate::FormatSql),
                            ))
                            .child(toolbar_button(
                                "btn-explain",
                                IconName::ListTree,
                                "Explain",
                                "Explain (⌥⌘E)",
                                Box::new(crate::Explain),
                            ))
                            .child(
                                zbutton("btn-run-query")
                                    .h(px(crate::ui::SMALL_H))
                                    .px_2p5()
                                    .ml_1()
                                    .icon(IconName::Play)
                                    .label(if is_running { "Running…" } else { "Run" })
                                    .tooltip(crate::ui::format_keys(
                                        "Run the selection, or the statement at the cursor (⌘↵) · ⌥↵ for actions",
                                    ))
                                    .primary()
                                    .small()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.refresh_active_tab(window, cx)
                                    })),
                            ),
                    ),
            )
            // The left padding is the ▶ column, so the button never sits on the line numbers
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pl(px(RUN_COL_W))
                    .bg(theme.bg_app)
                    // ⌘ scroll sizes the editor text (the interface keeps its own size). Caught in the
                    // capture phase: the editor would otherwise scroll and swallow it first
                    .child({
                        let this = cx.entity().downgrade();
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let this = this.clone();
                                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, _, cx| {
                                    if phase == DispatchPhase::Capture
                                        && e.modifiers.platform
                                        && bounds.contains(&e.position)
                                    {
                                        cx.stop_propagation();
                                        this.update(cx, |this, cx| this.zoom_editor_by_scroll(e, cx))
                                            .ok();
                                    }
                                });
                            },
                        )
                        .absolute()
                        .size_full()
                    })
                    // Escape is bound app-wide to Dismiss, so it arrives as that action, not a key
                    .when(vim_mode.is_some(), |d| {
                        d.capture_action(cx.listener(|this, _: &crate::Dismiss, window, cx| {
                            if this.vim_escape(window, cx) {
                                cx.stop_propagation();
                            }
                        }))
                    })
                    // Capture phase: the editor binds keys itself and would act on them first
                    .when(vim_mode.is_some(), |d| {
                        d.capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if this.handle_vim_key(event, window, cx) {
                                cx.stop_propagation();
                            }
                        }))
                    })
                    .child(
                        Editor::new(editor)
                            .h_full()
                            .bordered(false),
                    )
                    // The editor only draws a bar; the block goes on top, at the caret it laid out
                    .when_some(block_cursor, |d, w| {
                        let editor = editor.clone();
                        let color = Theme::tint(theme.accent, 0.5);
                        let app_bg: Hsla = theme.bg_app.into();
                        d.child(
                            canvas(
                                |_, _, _| {},
                                move |_, _, window, cx| {
                                    let st = editor.read(cx);
                                    if !st.focus_handle(cx).is_focused(window) {
                                        return;
                                    }
                                    if let Some((caret, _)) = st.cursor_layout() {
                                        // One cell of the editor's actual mono font, whatever the user picked
                                        let ts = window.text_system();
                                        let font_id = ts.resolve_font(&font(cx.theme().mono_font_family.clone()));
                                        let w = ts.advance(font_id, px(w), 'm').map(|a| a.width).unwrap_or(px(w * 0.6));
                                        // The editor still paints its blinking bar here: cover it with what's beneath
                                        let kit = cx.theme();
                                        window.paint_quad(fill(
                                            caret,
                                            kit.highlight_theme.style.editor_background.unwrap_or(app_bg),
                                        ));
                                        if let Some(line) = kit.highlight_theme.style.editor_active_line {
                                            window.paint_quad(fill(caret, line));
                                        }
                                        let size = size(w, caret.size.height);
                                        window.paint_quad(fill(Bounds::new(caret.origin, size), color));
                                    }
                                },
                            )
                            .absolute()
                            .size_full(),
                        )
                    }),
            )
            .children(run_gutter_buttons(
                editor, is_running, soft_wrap, rules, theme, cx,
            ))
    }
}

pub fn env_color(env: Environment, theme: &Theme) -> Rgba {
    match env {
        Environment::Local => theme.success,
        Environment::Staging => theme.warning,
        Environment::Prod => theme.danger,
    }
}

/// ▶ in the gutter on the first line of every statement in view; clicking one moves the cursor there and runs it.
/// Rows are placed from the editor's line height and scroll offset, which only holds while every line above is
/// one row high; with soft wrap on, a statement is skipped when a line above it might wrap.
/// ponytail: "might wrap" guesses a monospace char at 0.45 × line height; exact needs per-line layout from gpui-kit
fn run_gutter_buttons(
    editor: &Entity<EditorState>,
    is_running: bool,
    soft_wrap: bool,
    rules: quarry_sql::SplitRules,
    theme: &Theme,
    cx: &Context<QuarryWindow>,
) -> Vec<AnyElement> {
    let e = editor.read(cx);
    let Some(line_height) = e.line_height() else {
        return vec![];
    };
    let scroll_y = e.scroll_offset().y;
    let sql = e.value();
    // Large SQL scripts (e.g. bulk data loads) would otherwise re-parse thousands of statements every frame
    if sql.len() > 256_000 {
        return vec![];
    }
    let area = e.input_bounds();
    let starts: Vec<usize> = if rules.go_batches || rules.slash_blocks {
        quarry_sql::split_statements_with(&sql, rules)
            .into_iter()
            .map(|s| s.start)
            .collect()
    } else {
        quarry_sql::tree_sitter_split_statements(&sql)
            .into_iter()
            .map(|s| s.start)
            .collect()
    };
    let lines: Vec<usize> = if soft_wrap {
        sql.split('\n').map(|l| l.chars().count()).collect()
    } else {
        Vec::new()
    };
    let wrap_cols = (area.size.width / (line_height * 0.45)).floor() as usize;
    let fits = |line: usize| {
        !soft_wrap
            || lines[..line.min(lines.len())]
                .iter()
                .all(|n| *n < wrap_cols)
    };
    let x = area.origin.x - px(RUN_COL_W) + px(3.0);
    let mut line = 0usize;
    let mut last_pos = 0usize;
    starts
        .into_iter()
        .enumerate()
        .filter_map(|(ix, start)| {
            let clamped = start.min(sql.len());
            if clamped > last_pos {
                line += sql[last_pos..clamped].matches('\n').count();
                last_pos = clamped;
            }
            if !fits(line) {
                return None;
            }
            let y = area.origin.y + scroll_y + line_height * line as f32;
            if y < area.origin.y || y + line_height > area.origin.y + area.size.height {
                return None; // scrolled out of view
            }
            let editor = editor.clone();
            let button = div()
                .id(("gutter-run", ix))
                .w(px(14.0))
                .h(line_height)
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.text_muted)
                .when(!is_running, |d| d.hover(|s| s.text_color(theme.success)))
                .cursor_pointer()
                .tooltip(|window, cx| {
                    gpui::component::tooltip::Tooltip::new(crate::ui::format_keys(
                        "Run statement (⌘↵)",
                    ))
                    .build(window, cx)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    editor.update(cx, |e, cx| e.set_selected_range(start..start, cx));
                    this.refresh_active_tab(window, cx)
                }))
                .child(Icon::new(IconName::Play).xsmall());
            Some(deferred(anchored().position(point(x, y)).child(button)).into_any_element())
        })
        .collect()
}

fn toolbar_button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    tip: &'static str,
    action: Box<dyn Action>,
) -> impl IntoElement {
    zbutton(id)
        .h(px(crate::ui::SMALL_H))
        .px_2()
        .icon(icon)
        .label(label)
        .ghost()
        .small()
        .tooltip(crate::ui::format_keys(tip))
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}
