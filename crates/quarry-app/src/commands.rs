// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Overlay dismissal, menu navigation and the command bar's actions

use super::*;

impl QuarryWindow {
    /// Escape: close the topmost overlay
    pub(crate) fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if cx.has_active_drag() {
            cx.stop_active_drag(window);
            self.active_drag_pane = None;
            self.active_drag_tab = None;
            self.active_drag_split_pane = None;
            cx.notify();
            return;
        }
        if self.tab_switcher.take().is_some() {
            cx.notify();
            return;
        }
        if self.pending_crash_report.take().is_some() {
            crash_reporter::acknowledge_last_crash();
            cx.notify();
            return;
        }
        if self.state.renaming_query.is_some() {
            return self.finish_rename(None, window, cx);
        }
        if std::mem::take(&mut self.confirm_close_open) {
            cx.notify();
            return;
        }
        if std::mem::take(&mut self.settings_open) {
            cx.notify();
            return;
        }
        if self.pending_prod_run.take().is_some() || self.panel.confirm_kill.take().is_some() {
            cx.notify();
            return;
        }
        if self.params.take().is_some() {
            self.refocus_root(window, cx);
            cx.notify();
            return;
        }
        if self.filter_builder.take().is_some() {
            self.refocus_root(window, cx);
            cx.notify();
            return;
        }
        if self.export.open {
            self.export.open = false;
            cx.notify();
            return;
        }
        if self.import.open {
            // A running import is cancelled from its own button, not Esc
            self.import.open = self.import_busy();
            cx.notify();
            return;
        }
        let s = &mut self.state;
        let Some(open) = [
            &mut s.command_bar_open,
            &mut s.review_modal_open,
            &mut s.add_conn_modal_open,
        ]
        .into_iter()
        .find(|open| **open) else {
            return;
        };
        *open = false;
        self.refocus_root(window, cx);
        cx.notify();
    }

    pub(crate) fn menu_up(&mut self, _: &MenuUp, _: &mut Window, cx: &mut Context<Self>) {
        self.state.command_bar_selected_index =
            self.state.command_bar_selected_index.saturating_sub(1);
        cx.notify();
    }

    pub(crate) fn menu_down(&mut self, _: &MenuDown, _: &mut Window, cx: &mut Context<Self>) {
        let count =
            CommandBar::get_candidates(&self.state, &self.command_input.read(cx).value()).len();
        if self.state.command_bar_selected_index + 1 < count {
            self.state.command_bar_selected_index += 1;
        }
        cx.notify();
    }

    pub(crate) fn confirm_command_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let candidates =
            CommandBar::get_candidates(&self.state, &self.command_input.read(cx).value());
        let idx = self
            .state
            .command_bar_selected_index
            .min(candidates.len().saturating_sub(1));
        if let Some(item) = candidates.get(idx) {
            self.handle_command_action(item.id.clone(), window, cx);
        }
    }

    /// Handles actions triggered from the command bar
    pub fn handle_command_action(
        &mut self,
        action_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.command_bar_open = false;
        self.refocus_root(window, cx);
        if let Some(ix) = action_id
            .strip_prefix("usage:")
            .and_then(|i| i.parse::<usize>().ok())
        {
            return self.open_usage(ix, window, cx);
        }
        self.state.usages = None;

        if let Some((schema, table)) = action_id
            .strip_prefix("table:")
            .and_then(|t| t.split_once('\u{1f}'))
        {
            self.open_table_tab(table, schema, window, cx);
            self.refresh_active_tab(window, cx);
        } else if let Some((view, rest)) = action_id
            .strip_prefix("table-")
            .and_then(|r| r.split_once(':'))
        {
            if let Some((schema, table)) = rest.split_once('\u{1f}') {
                self.open_pin(
                    pins::Pin::Table {
                        schema: schema.into(),
                        table: table.into(),
                    },
                    window,
                    cx,
                );
                let view = if view == "ddl" {
                    table_detail::TableView::Ddl
                } else {
                    table_detail::TableView::Structure
                };
                self.set_table_view(view, window, cx);
            }
        } else if let Some(ix) = action_id
            .strip_prefix("pin:")
            .and_then(|i| i.parse::<usize>().ok())
        {
            let pin = self
                .state
                .active_connection
                .as_ref()
                .and_then(|c| self.state.pins.get(&c.id))
                .and_then(|p| p.get(ix))
                .cloned();
            if let Some(pin) = pin {
                self.open_pin(pin, window, cx);
            }
        } else if let Some(id) = action_id.strip_prefix("connection:") {
            self.connect_saved(id.to_string(), window, cx);
        } else if let Some(ix) = action_id
            .strip_prefix("history:")
            .and_then(|i| i.parse::<usize>().ok())
        {
            if let Some(sql) = self.state.history.get(ix).map(|h| h.sql.clone()) {
                self.open_query_tab(&sql, window, cx);
            }
        } else if let Some(query) = action_id.strip_prefix("run:") {
            self.open_query_tab(query, window, cx);
            self.refresh_active_tab(window, cx);
        } else if action_id == "action:maximize" {
            self.state.toggle_maximize();
        } else if action_id == "action:new_query" {
            self.new_query(&NewQuery, window, cx);
        } else if action_id == "action:review" {
            self.review_changes(&ReviewChanges, window, cx);
        } else if action_id == "action:compare_schema" {
            self.open_compare_menu(window, cx);
        } else if action_id == "action:add_conn" {
            self.open_add_conn(window, cx);
        } else if action_id == "action:toggle_sidebar" {
            self.toggle_sidebar(&ToggleSidebar, window, cx);
        } else if action_id == "action:explain" {
            self.start_run(RunMode::Explain, window, cx);
        } else if action_id == "action:explain_analyze" {
            self.start_run(RunMode::ExplainAnalyze, window, cx);
        } else if action_id == "action:settings" {
            self.open_settings(cx);
        } else if action_id == "action:keymap" {
            self.open_config_file(true, window, cx);
        } else if action_id == "action:pin" {
            self.pin_current_tab(cx);
        } else if action_id == "action:close_all" {
            self.close_all_tabs(cx);
            self.refocus_root(window, cx);
        } else if action_id == "action:split" {
            self.toggle_split(cx);
        } else if action_id == "action:toggle_split_direction" {
            self.toggle_split_direction(cx);
        } else if action_id == "action:focus_other_pane" {
            self.swap_split(cx);
        } else if action_id == "action:swap_split_panes" {
            self.swap_pane_positions(cx);
        } else if action_id == "action:move_pane_left" {
            self.move_pane_left(cx);
        } else if action_id == "action:move_pane_right" {
            self.move_pane_right(cx);
        } else if action_id == "action:move_pane_up" {
            self.move_pane_up(cx);
        } else if action_id == "action:move_pane_down" {
            self.move_pane_down(cx);
        } else if action_id == "action:reopen_tab" {
            self.reopen_closed_tab(&ReopenClosedTab, window, cx);
        } else if action_id == "action:format" {
            self.format_query(window, cx);
        } else if action_id == "action:open_file" {
            self.open_sql_file(window, cx);
        } else if action_id == "action:run_file" {
            self.run_sql_file(window, cx);
        } else if action_id == "action:save_file" {
            self.save_query(false, window, cx);
        } else if action_id == "action:export" {
            self.open_export(cx);
        } else if action_id == "action:import" {
            self.open_import(window, cx);
        } else if action_id == "action:dump" {
            self.dump_database(window, cx);
        } else if action_id == "action:theme" {
            self.toggle_theme(&ToggleTheme, window, cx);
        } else if action_id == "action:increase_font" {
            self.increase_font_size(&IncreaseFontSize, window, cx);
        } else if action_id == "action:decrease_font" {
            self.decrease_font_size(&DecreaseFontSize, window, cx);
        } else if action_id == "action:reset_font" {
            self.reset_font_size(&ResetFontSize, window, cx);
        } else if action_id == "action:toggle_tx" {
            self.toggle_tx_mode(&ToggleTxMode, window, cx);
        } else if action_id == "action:commit_tx" {
            self.commit_tx(&CommitTx, window, cx);
        } else if action_id == "action:rollback_tx" {
            self.rollback_tx(&RollbackTx, window, cx);
        } else if action_id == "action:toggle_bottom" {
            self.toggle_bottom_panel(&ToggleBottomPanel, window, cx);
        } else if action_id == "action:move_sidebar_left" {
            self.move_sidebar(layout::Side::Left, window, cx);
        } else if action_id == "action:move_sidebar_right" {
            self.move_sidebar(layout::Side::Right, window, cx);
        } else if action_id == "action:move_bottom_bottom" {
            self.move_bottom_panel(layout::Side::Bottom, window, cx);
        } else if action_id == "action:move_bottom_right" {
            self.move_bottom_panel(layout::Side::Right, window, cx);
        } else if action_id == "action:reset_layout" {
            self.reset_layout(window, cx);
        } else if action_id == "action:report_issue" {
            cx.open_url("https://github.com/thekodeking/quarry/issues/new");
        } else if action_id == "action:view_last_crash" {
            self.view_last_crash(cx);
        }

        cx.notify();
    }
}
