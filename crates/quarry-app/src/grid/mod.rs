// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Interactive data grid component, cell editors, aggregates, and table source paging.
//!
//! Provides the [`ResultSet`] data delegate backing GPUI data tables, managing in-place cell editing,
//! multi-cell selection ranges, statistical aggregates (sum/avg/min/max/count), transposition,
//! undo/redo stacks for staged edits, foreign-key navigations, and quick filtering.

use crate::runtime::run_db;
use crate::theme::Theme;
use crate::ui::*;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, MoveDown, MoveUp, Position, Textarea,
    TextareaState,
};
use gpui::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui::component::resizable::{h_resizable, resizable_panel};
use gpui::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui::component::tooltip::Tooltip;
use gpui::component::{ActiveTheme, Disableable, Icon, Sizable};
use gpui::*;
use quarry_core::edits::{PendingChanges, PendingRowInsert, RowKey};
use quarry_core::schema::{ColumnSchema, ColumnType};
use quarry_core::session::{DatabaseSession, QueryResult};
use quarry_core::value::DbValue;
use std::sync::Arc;

mod actions;
mod analytics;
mod delegate;
mod editing;

pub use actions::*;
pub use analytics::*;
pub use editing::{Fetch, PageTo};

/// Formats a cell value lazily for the value inspector, capping pretty-printing at 256 KB.
pub fn format_cell_preview(raw: &str) -> (String, &'static str) {
    if raw.len() > 256 * 1024 {
        return (
            format!("[Large value: {} bytes — use Copy to inspect]", raw.len()),
            "text",
        );
    }
    let trimmed = raw.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Ok(pretty) = serde_json::to_string_pretty(&val) {
                return (pretty, "json");
            }
        }
    }
    (raw.to_string(), "text")
}

actions!(
    grid,
    [
        EditCell,
        CancelEdit,
        NextCell,
        PrevCell,
        AddRow,
        DuplicateRow,
        DeleteRow,
        SetNull,
        SetDefault,
        ToggleValueEditor,
        ToggleTransposedView,
        TransposedNextRow,
        TransposedPrevRow,
        ApplyValue,
        SubmitChanges,
        RevertChanges,
        UndoChange,
        RedoChange,
        CopyValue,
        CopyRowTsv,
        CopyRowCsv,
        CopyRowJson,
        CopyRowInsert,
        CopyAllTsv,
        CopyAllCsv,
        CopyAllJson,
        CopyAllInsert,
        HideColumn,
        ShowAllColumns,
        FollowForeignKey,
        FilterByValue,
        FilterExcludeValue,
        ApplyPendingFilter,
        OpenFilterBuilder,
        SelectRangeUp,
        SelectRangeDown,
        SelectRangeLeft,
        SelectRangeRight
    ]
);

/// DataGrip-style grid keys; scoped to the grid so ⌘N/⌘↵ keep their global meaning elsewhere.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", EditCell, Some("DataTable")),
        KeyBinding::new("f2", EditCell, Some("DataTable")),
        KeyBinding::new("escape", CancelEdit, Some("DataTable > Input")),
        KeyBinding::new("tab", NextCell, Some("DataTable > Input")),
        KeyBinding::new("shift-tab", PrevCell, Some("DataTable > Input")),
        KeyBinding::new("secondary-enter", SubmitChanges, Some("DataTable > Input")),
        KeyBinding::new("secondary-alt-n", SetNull, Some("DataTable > Input")),
        KeyBinding::new("secondary-n", AddRow, Some("DataTable")),
        KeyBinding::new("secondary-d", DuplicateRow, Some("DataTable")),
        KeyBinding::new("secondary-backspace", DeleteRow, Some("DataTable")),
        KeyBinding::new("backspace", SetNull, Some("DataTable")),
        KeyBinding::new("delete", SetNull, Some("DataTable")),
        KeyBinding::new("secondary-alt-n", SetNull, Some("DataTable")),
        // Not ⌥⌘D: macOS reserves it for showing/hiding the Dock
        KeyBinding::new("secondary-shift-d", SetDefault, Some("DataTable")),
        KeyBinding::new("shift-enter", ToggleValueEditor, Some("DataTable")),
        KeyBinding::new("alt-r", ToggleTransposedView, Some("DataTable")),
        KeyBinding::new("secondary-alt-f", OpenFilterBuilder, Some("DataTable")),
        KeyBinding::new("shift-up", SelectRangeUp, Some("DataTable")),
        KeyBinding::new("shift-down", SelectRangeDown, Some("DataTable")),
        KeyBinding::new("shift-left", SelectRangeLeft, Some("DataTable")),
        KeyBinding::new("shift-right", SelectRangeRight, Some("DataTable")),
        // Registered after the app's Input bindings, so ⌘↵ applies in the value editor instead of running a query
        KeyBinding::new("secondary-enter", ApplyValue, Some("ValueEditor > Input")),
        KeyBinding::new("secondary-enter", SubmitChanges, Some("DataTable")),
        KeyBinding::new("secondary-alt-z", RevertChanges, Some("DataTable")),
        // Inside the cell editor ⌘Z is the text's own undo; on the grid it steps through pending changes
        KeyBinding::new("secondary-z", UndoChange, Some("DataTable")),
        KeyBinding::new("secondary-shift-z", RedoChange, Some("DataTable")),
        KeyBinding::new("secondary-c", CopyValue, Some("DataTable")),
        KeyBinding::new("secondary-shift-c", CopyRowTsv, Some("DataTable")),
        KeyBinding::new("f4", FollowForeignKey, Some("DataTable")),
    ]);
}

/// Where a table tab's rows come from; enables paging, server-side sort and editing.
pub struct TableSource {
    pub session: Arc<dyn DatabaseSession>,
    pub schema: String,
    pub table: String,
    pub columns: Vec<ColumnSchema>,
    pub primary_keys: Vec<String>,
    /// Why this table can't be edited, if it can't (E1, E10)
    pub read_only_reason: Option<&'static str>,
    /// Columns that reference other tables (G5)
    pub foreign_keys: Vec<quarry_core::schema::ForeignKeyRef>,
    /// Rows per page (settings: rows_per_page)
    pub page_size: usize,
    pub filter: String,
    pub order_by: Option<(String, bool)>, // (column, descending)
    /// First row of the page on screen (0-based); pages replace each other, like DataGrip
    pub offset: usize,
    /// Rows matching the filter, once counted (or known from reaching the last page)
    pub total: Option<u64>,
    pub counting: bool,
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
}

impl TableSource {
    pub fn dialect(&self) -> &'static dyn quarry_core::Dialect {
        self.session.dialect()
    }

    pub fn qualified_name(&self) -> String {
        self.dialect().qualified(&self.schema, &self.table)
    }

    /// The tab's query (filter + sort), without paging
    pub fn select_sql(&self) -> String {
        let mut sql = format!("SELECT * FROM {}", self.qualified_name());
        if !self.filter.trim().is_empty() {
            sql += &format!(" WHERE {}", self.filter);
        }
        if let Some((col, desc)) = &self.order_by {
            sql += &format!(
                " ORDER BY {}{}",
                self.dialect().quote_ident(col),
                if *desc { " DESC" } else { "" }
            );
        }
        sql
    }

    /// `SELECT COUNT(*)` over the tab's filter
    fn count_sql(&self) -> String {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.qualified_name());
        if !self.filter.trim().is_empty() {
            sql += &format!(" WHERE {}", self.filter);
        }
        sql
    }

    fn page_sql(&self, offset: usize, limit: usize) -> String {
        // One extra row tells us whether another page exists
        self.dialect()
            .limit(&self.select_sql(), limit as u64 + 1, offset as u64)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InspectorMode {
    Tree,
    Text,
}

pub struct Inspector {
    pub row: usize,
    pub col: usize,
    pub editor: Entity<EditorState>,
    pub initial: String,
    pub mode: InspectorMode,
    /// Shared so each frame's render doesn't deep-copy a large document
    pub parsed_json: Option<std::rc::Rc<serde_json::Value>>,
    pub expanded_paths: std::collections::HashSet<String>,
    pub search_input: Option<Entity<InputState>>,
    /// Tree view: the leaf being edited in place (JSONPath, its input)
    tree_edit: Option<(String, Entity<InputState>, Subscription)>,
    /// Text view soft wrap; off by default since long lines re-wrap on every resize step
    pub wrap: bool,
}

/// Text edits in a textarea that grows and pops out; a number in a one-line input, which
/// (unlike the textarea) right-aligns, so the digits stay where the grid drew them
enum CellInput {
    Text(Entity<TextareaState>),
    Number(Entity<InputState>),
}

impl CellInput {
    fn value(&self, cx: &App) -> String {
        match self {
            CellInput::Text(e) => e.read(cx).value().to_string(),
            CellInput::Number(e) => e.read(cx).value().to_string(),
        }
    }
}

struct CellEditor {
    row: usize,
    col: usize,
    display_col: usize,
    input: CellInput,
    /// Cell width from the last paint; the editor pops out over the grid when the text won't fit
    cell_w: std::rc::Rc<std::cell::Cell<Pixels>>,
    initial: String,
    error: Option<String>,
    /// Date and timestamp cells: ↑/↓ step the part under the caret, a strip reads the value back
    date: Option<DatePick>,
    _sub: Subscription,
}

struct DatePick {
    /// Timestamps carry a time after the date; "Now" fills both
    timestamp: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum RowState {
    Loaded,
    Inserted,
    Deleted,
}

/// One or more result sets plus pending edits; the model behind every grid.
pub struct ResultSet {
    /// One entry per row-returning statement; shown as sub-tabs when there are several
    pub results: Vec<QueryResult>,
    pub active: usize,
    pub pending: PendingChanges,
    /// Earlier and undone states of `pending`, for ⌘Z / ⇧⌘Z until the changes are submitted
    undo: Vec<PendingChanges>,
    redo: Vec<PendingChanges>,
    pub source: Option<TableSource>,
    /// Last validation or commit problem, shown in the toolbar until the next change
    pub notice: Option<String>,
    editing: Option<CellEditor>,
    /// The currently selected cell (row, col)
    pub selected_cell: Option<(usize, usize)>,
    /// Selected range spanning one or more cells (display rows and display columns)
    pub selected_range: Option<CellRange>,
    /// Whether user is currently dragging mouse across cells to select a range
    pub is_dragging: bool,
    /// Side panel for long text and JSON ("value editor"); follows the selected cell
    inspector: Option<Inspector>,
    /// Display order of columns (actual indices); hidden columns are left out. `None` = natural order
    order: Option<Vec<usize>>,
    colors: CellColors,
    /// From settings: how NULL reads and the widest an auto-sized column gets
    null_label: SharedString,
    max_col_width: f32,
    /// The column being sorted (by name) and how; the header icon is rebuilt from this
    sort: Option<(String, ColumnSort)>,
    /// Server logs: rows whose value in this column is ERROR/FATAL/PANIC read in red
    pub level_column: Option<usize>,
    /// Right-click menu on a column header: where, the menu, and its dismiss subscription
    header_menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    /// Currently active streaming statement index
    streaming_stmt: Option<usize>,
    /// Pending filter clause to be applied to the active table tab
    pub pending_filter_clause: Option<String>,
    /// Column to preselect when opening the filter builder from a header
    pub pending_filter_builder_col: Option<String>,
    /// Whether transposed row view is active (viewing a single row vertically)
    pub transposed_view: bool,
    /// Currently displayed row index in transposed view
    pub transposed_row: usize,
    /// Search input for filtering columns in transposed view
    pub transposed_search_input: Option<Entity<InputState>>,
}

#[derive(Clone, Copy)]
struct CellColors {
    text: Rgba,
    muted: Rgba,
    token_bg: Rgba,
    success: Rgba,
    danger: Rgba,
    pending_bg: Rgba,
    inserted_bg: Rgba,
    deleted_bg: Rgba,
    accent: Rgba,
    editor_bg: Rgba,
    /// Text on a success/danger fill
    on_fill: Rgba,
    grid_line: Hsla,
}

impl From<&Theme> for CellColors {
    fn from(theme: &Theme) -> Self {
        Self {
            text: theme.text_primary,
            muted: theme.text_muted,
            token_bg: theme.bg_surface,
            success: theme.success,
            danger: theme.danger,
            pending_bg: theme.pending_cell_bg,
            inserted_bg: Theme::tint(theme.success, 0.15).into(),
            deleted_bg: Theme::tint(theme.danger, 0.15).into(),
            on_fill: theme.on_accent(),
            grid_line: theme.grid_line(),
            accent: theme.accent,
            editor_bg: if theme.is_dark {
                theme.bg_surface
            } else {
                theme.bg_app
            },
        }
    }
}

impl ResultSet {
    pub fn set_theme(&mut self, theme: &Theme) {
        self.colors = CellColors::from(theme);
    }

    pub fn set_prefs(&mut self, null_label: &str, max_col_width: f32) {
        self.null_label = null_label.to_string().into();
        self.max_col_width = max_col_width;
    }

    pub fn new(theme: &Theme, source: Option<TableSource>) -> Self {
        Self {
            results: Vec::new(),
            active: 0,
            pending: PendingChanges::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            source,
            notice: None,
            editing: None,
            selected_cell: None,
            selected_range: None,
            is_dragging: false,
            inspector: None,
            order: None,
            null_label: "NULL".into(),
            max_col_width: 420.0,
            sort: None,
            level_column: None,
            header_menu: None,
            streaming_stmt: None,
            pending_filter_clause: None,
            pending_filter_builder_col: None,
            transposed_view: false,
            transposed_row: 0,
            transposed_search_input: None,
            colors: CellColors::from(theme),
        }
    }

    pub fn result(&self) -> Option<&QueryResult> {
        self.results.get(self.active)
    }

    pub fn set_results(&mut self, results: Vec<QueryResult>) {
        // Keep the user's column order/hiding across reloads of the same shape
        let same_columns = self.result().map(|r| &r.columns) == results.first().map(|r| &r.columns);
        if !same_columns {
            self.order = None;
        }
        self.results = results;
        self.active = 0;
        self.editing = None;
        self.selected_cell = None;
        self.selected_range = None;
        self.is_dragging = false;
        self.transposed_view = false;
        self.transposed_row = 0;
        self.streaming_stmt = None;
        // New query results arrive unsorted; table tabs keep sorting on the server
        if self.source.is_none() {
            self.sort = None;
        }
        self.inspector = None;
    }

    /// Receives a streaming chunk for statement `chunk.statement_index`.
    /// If this is the first chunk for the statement, initializes or updates the corresponding `QueryResult`.
    /// If subsequent chunk, appends rows to the existing `QueryResult`.
    pub fn append_chunk(&mut self, chunk: quarry_core::QueryChunk) {
        if chunk.columns.is_empty() {
            return;
        }
        let is_new = self.streaming_stmt != Some(chunk.statement_index);
        if is_new {
            self.streaming_stmt = Some(chunk.statement_index);
            let same_columns = self.result().map(|r| &r.columns) == Some(&chunk.columns);
            if !same_columns && self.results.is_empty() {
                self.order = None;
            }
            if self.results.is_empty() {
                self.active = 0;
                self.editing = None;
                self.selected_cell = None;
                self.selected_range = None;
                self.is_dragging = false;
                self.transposed_view = false;
                self.transposed_row = 0;
                if self.source.is_none() {
                    self.sort = None;
                }
                self.inspector = None;
            }
            self.results.push(QueryResult {
                columns: chunk.columns,
                rows: chunk.rows,
                rows_affected: chunk.rows_affected,
                execution_time: chunk.execution_time,
                statement: chunk.statement,
            });
        } else if let Some(last) = self.results.last_mut() {
            last.rows.append(&chunk.rows);
            last.rows_affected = chunk.rows_affected;
            last.execution_time = chunk.execution_time;
        }

        if chunk.is_last {
            self.streaming_stmt = None;
        }
    }

    /// Switches between result sub-tabs
    pub fn set_active(&mut self, ix: usize) {
        self.active = ix;
        self.order = None;
        self.editing = None;
        self.selected_cell = None;
        self.selected_range = None;
        self.is_dragging = false;
        self.transposed_view = false;
        self.transposed_row = 0;
        self.inspector = None;
    }

    /// Actual column index for a displayed column
    pub fn actual(&self, display: usize) -> usize {
        self.order
            .as_ref()
            .and_then(|o| o.get(display).copied())
            .unwrap_or(display)
    }

    /// Displayed columns, as actual indices, in display order
    fn visible(&self) -> Vec<usize> {
        self.order
            .clone()
            .unwrap_or_else(|| (0..self.result().map_or(0, |r| r.columns.len())).collect())
    }

    pub fn hide_column(&mut self, display: usize) {
        let mut order = self.visible();
        if order.len() > 1 && display < order.len() {
            order.remove(display);
            self.order = Some(order);
        }
    }

    pub fn show_all_columns(&mut self) {
        self.order = None;
    }

    pub fn toggle_transposed_view(&mut self, window: &mut Window, cx: &mut App) {
        self.transposed_view = !self.transposed_view;
        let total_rows = self.total_row_count();
        if self.transposed_view {
            if let Some((r, _)) = self.selected_cell {
                self.transposed_row = r.min(total_rows.saturating_sub(1));
            } else if total_rows > 0 {
                self.transposed_row = 0;
                self.selected_cell = Some((0, 0));
            }
            if self.transposed_search_input.is_none() {
                self.transposed_search_input =
                    Some(cx.new(|cx| InputState::new(window, cx).placeholder("Filter columns…")));
            }
        } else if let Some((_, c)) = self.selected_cell {
            self.selected_cell = Some((self.transposed_row.min(total_rows.saturating_sub(1)), c));
        }
    }

    pub fn transposed_next_row(&mut self) {
        let total = self.total_row_count();
        if total > 0 && self.transposed_row + 1 < total {
            self.transposed_row += 1;
            let c = self.selected_cell.map(|(_, c)| c).unwrap_or(0);
            self.selected_cell = Some((self.transposed_row, c));
        }
    }

    pub fn transposed_prev_row(&mut self) {
        if self.transposed_row > 0 {
            self.transposed_row -= 1;
            let c = self.selected_cell.map(|(_, c)| c).unwrap_or(0);
            self.selected_cell = Some((self.transposed_row, c));
        }
    }

    pub fn hidden_count(&self) -> usize {
        self.result().map_or(0, |r| r.columns.len()) - self.visible().len()
    }

    /// The table a foreign-key cell points at, and a WHERE condition for its row
    pub fn foreign_key_target(
        &self,
        row: usize,
        display: usize,
    ) -> Option<(String, String, String)> {
        let col = self.actual(display);
        let name = self.column_name(col)?;
        let src = self.source.as_ref()?;
        let fk = src.foreign_keys.iter().find(|f| f.from_column == name)?;
        let value = self.cell_value(row, col).0.filter(|v| !v.is_null())?;
        let filter = format!(
            "{} = {}",
            src.dialect().quote_ident(&fk.to_column),
            src.dialect().literal(&value)
        );
        Some((fk.to_schema.clone(), fk.to_table.clone(), filter))
    }

    /// Copies one row (or all rows) of visible columns in the given format
    pub fn copy_text(
        &self,
        row: Option<usize>,
        format: quarry_core::export::ExportFormat,
        delimiter: char,
    ) -> Option<String> {
        let r = self.result()?;
        let table = self
            .source
            .as_ref()
            .map(|s| s.qualified_name())
            .unwrap_or_else(|| "query_result".into());
        let opts = quarry_core::export::ExportOptions {
            format,
            delimiter,
            header: row.is_none(),
            null_text: String::new(),
            columns: Some(self.visible()),
            table,
        };
        let mut buf = Vec::new();
        let mut ex = quarry_core::export::Exporter::begin(&mut buf, &r.columns, opts).ok()?;
        match row {
            Some(row) => {
                let single = vec![(0..r.columns.len())
                    .map(|c| self.cell_value(row, c).0.unwrap_or(DbValue::Null))
                    .collect()];
                ex.rows(&single).ok()?;
            }
            None => {
                ex.row_buffer(&r.rows).ok()?;
            }
        }
        ex.finish().ok()?;
        String::from_utf8(buf)
            .ok()
            .map(|s| s.trim_end().to_string())
    }

    /// One row as `column<TAB>value` lines, or as a pretty JSON object in column order
    pub fn row_pairs_text(&self, row: usize, json: bool) -> String {
        let lines: Vec<String> = self
            .visible()
            .into_iter()
            .map(|c| {
                let name = self.column_name(c).unwrap_or("?");
                let v = self.cell_value(row, c).0.unwrap_or(DbValue::Null);
                if json {
                    let key = serde_json::Value::String(name.into());
                    format!("  {key}: {}", quarry_core::export::json_value(&v))
                } else if v.is_null() {
                    format!("{name}\t")
                } else {
                    format!("{name}\t{}", v.to_display_string())
                }
            })
            .collect();
        if json {
            format!("{{\n{}\n}}", lines.join(",\n"))
        } else {
            lines.join("\n")
        }
    }

    pub fn read_only_reason(&self) -> Option<&'static str> {
        match &self.source {
            None => Some("Query results are read-only"),
            Some(src) => src.read_only_reason,
        }
    }

    fn loaded_len(&self) -> usize {
        self.result().map_or(0, |r| r.rows.len())
    }

    fn column_name(&self, col: usize) -> Option<&str> {
        self.result()?.columns.get(col).map(String::as_str)
    }

    fn column_schema(&self, col: usize) -> Option<&ColumnSchema> {
        let name = self.column_name(col)?;
        self.source
            .as_ref()?
            .columns
            .iter()
            .find(|c| c.name == name)
    }

    /// Primary-key values of a loaded row, as loaded
    fn row_key(&self, row: usize) -> Option<RowKey> {
        let (src, r) = (self.source.as_ref()?, self.result()?);
        let values = r.rows.get(row)?;
        src.primary_keys
            .iter()
            .map(|pk| {
                Some((
                    pk.clone(),
                    values.get(r.columns.iter().position(|c| c == pk)?)?.clone(),
                ))
            })
            .collect()
    }

    /// Sorts by display column `col_ix`; `Default` restores the original order
    fn apply_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let col_ix = self.actual(col_ix);
        let Some(name) = self.result().and_then(|r| r.columns.get(col_ix)).cloned() else {
            return;
        };
        self.sort = (sort != ColumnSort::Default).then(|| (name.clone(), sort));
        // Rebuild the headers from `self.sort` once this update is done, so the icon shows what's applied
        cx.defer_in(window, |table, _, cx| table.refresh(cx));

        // Tables sort on the server (the full table, not just the loaded page), like DataGrip
        if let Some(src) = self.source.as_mut() {
            src.order_by = match sort {
                ColumnSort::Ascending => Some((name, false)),
                ColumnSort::Descending => Some((name, true)),
                ColumnSort::Default => None,
            };
            self.fetch(Fetch::Reset, window, cx);
            return;
        }
        let Some(r) = self.results.get_mut(self.active) else {
            return;
        };
        match sort {
            ColumnSort::Ascending => r.rows.sort_by(col_ix, |a, b| a.cmp_val(b)),
            ColumnSort::Descending => r.rows.sort_by(col_ix, |a, b| b.cmp_val(a)),
            ColumnSort::Default => r.rows.reset_order(),
        }
    }

    fn open_header_menu(
        &mut self,
        col: usize,
        pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let name = self
            .column_name(self.actual(col))
            .unwrap_or_default()
            .to_string();
        let grid = cx.entity().downgrade();
        let item = |label: &'static str,
                    f: fn(
            &mut ResultSet,
            usize,
            &mut Window,
            &mut Context<TableState<ResultSet>>,
        )| {
            let grid = grid.clone();
            gpui::component::menu::PopupMenuItem::new(label).on_click(move |_, window, cx| {
                grid.update(cx, |t, cx| {
                    f(t.delegate_mut(), col, window, cx);
                    t.refresh(cx);
                    cx.notify();
                })
                .ok();
            })
        };
        let can_hide = self.visible().len() > 1;
        let has_hidden = self.order.is_some();
        let is_table = self.source.is_some();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.item(item("Sort Ascending", |rs, c, w, cx| {
                rs.apply_sort(c, ColumnSort::Ascending, w, cx)
            }))
            .item(item("Sort Descending", |rs, c, w, cx| {
                rs.apply_sort(c, ColumnSort::Descending, w, cx)
            }))
            .item(item("Clear Sort", |rs, c, w, cx| {
                rs.apply_sort(c, ColumnSort::Default, w, cx)
            }))
            .when(is_table, |m| {
                m.separator()
                    .item(item("Filter: IS NULL", |rs, c, w, cx| {
                        rs.apply_header_null_filter(c, true, w, cx);
                    }))
                    .item(item("Filter: IS NOT NULL", |rs, c, w, cx| {
                        rs.apply_header_null_filter(c, false, w, cx);
                    }))
                    .item(item("Filter Builder…", |rs, c, w, cx| {
                        rs.open_filter_builder_for_col(c, w, cx);
                    }))
            })
            .separator()
            .when(can_hide, |m| {
                m.item(item("Hide Column", |rs, c, _, _| rs.hide_column(c)))
            })
            .when(has_hidden, |m| {
                m.item(item("Show All Columns", |rs, _, _, _| {
                    rs.show_all_columns()
                }))
            })
            .item(
                gpui::component::menu::PopupMenuItem::new("Copy Column Name").on_click(
                    move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(name.clone())),
                ),
            )
        });
        menu.update(cx, |m, cx| m.focus_handle(cx).focus(window, cx));
        let sub = cx.subscribe(&menu, |t, _, _: &DismissEvent, cx| {
            t.delegate_mut().header_menu = None;
            cx.notify();
        });
        self.header_menu = Some((pos, menu, sub));
        cx.notify();
    }

    pub fn filter_by_selected_cell(&self, equal: bool) -> Option<String> {
        let (row, col) = self.selected_cell?;
        if row >= self.loaded_len() {
            return None;
        }
        let col_ix = self.actual(col);
        let col_name = self.column_name(col_ix)?;
        let (val, _) = self.cell_value(row, col_ix);
        let src = self.source.as_ref()?;
        let quoted = src.dialect().quote_ident(col_name);
        match val {
            None | Some(DbValue::Null) => {
                if equal {
                    Some(format!("{quoted} IS NULL"))
                } else {
                    Some(format!("{quoted} IS NOT NULL"))
                }
            }
            Some(v) => {
                let op = if equal { "=" } else { "!=" };
                Some(format!("{quoted} {op} {}", v.to_sql_literal()))
            }
        }
    }

    pub fn apply_header_null_filter(
        &mut self,
        col: usize,
        is_null: bool,
        window: &mut Window,
        cx: &mut Context<TableState<ResultSet>>,
    ) {
        let col_ix = self.actual(col);
        if let (Some(col_name), Some(src)) = (self.column_name(col_ix), self.source.as_ref()) {
            let quoted = src.dialect().quote_ident(col_name);
            let op = if is_null { "IS NULL" } else { "IS NOT NULL" };
            self.pending_filter_clause = Some(format!("{quoted} {op}"));
            window.dispatch_action(Box::new(ApplyPendingFilter), cx);
        }
    }

    pub fn open_filter_builder_for_col(
        &mut self,
        col: usize,
        window: &mut Window,
        cx: &mut Context<TableState<ResultSet>>,
    ) {
        let col_ix = self.actual(col);
        if let Some(col_name) = self.column_name(col_ix) {
            self.pending_filter_builder_col = Some(col_name.to_string());
            window.dispatch_action(Box::new(OpenFilterBuilder), cx);
        }
    }

    fn is_error_row(&self, row: usize) -> bool {
        self.level_column.is_some_and(|col| {
            matches!(self.cell_value_ref(row, col).0, Some(DbValue::Text(ref l)) if matches!(l.as_str(), "ERROR" | "FATAL" | "PANIC"))
        })
    }

    fn row_state(&self, row: usize) -> RowState {
        if row >= self.loaded_len() {
            RowState::Inserted
        } else if self
            .row_key(row)
            .is_some_and(|k| self.pending.is_deleted(&k))
        {
            RowState::Deleted
        } else {
            RowState::Loaded
        }
    }

    /// Total number of rows currently in this result set (loaded + pending inserts).
    pub fn total_row_count(&self) -> usize {
        self.loaded_len()
            + if self.source.is_some() {
                self.pending.row_inserts.len()
            } else {
                0
            }
    }

    /// Computes summary statistics for a given column (actual index).
    pub fn compute_column_stats(&self, actual_col: usize) -> Option<ColumnStats> {
        let col_name = self.column_name(actual_col)?.to_string();
        let type_name = self
            .column_schema(actual_col)
            .map(|s| s.raw_type_name.to_uppercase())
            .unwrap_or_else(|| "ANY".into());
        let total = self.total_row_count();
        let iter = (0..total).map(|r| self.cell_value_ref(r, actual_col).0);
        Some(ColumnStats::compute(col_name, type_name, iter))
    }

    /// Handles mouse down on a cell (display coordinates).
    /// If Shift is held, extends from existing anchor; otherwise sets anchor and begins drag.
    pub fn handle_cell_mouse_down(&mut self, row: usize, col: usize, is_shift: bool) {
        if is_shift {
            let (anchor_r, anchor_c) = match self.selected_range {
                Some(r) => (r.anchor_row, r.anchor_col),
                None => self.selected_cell.unwrap_or((row, col)),
            };
            self.selected_range = Some(CellRange::new(anchor_r, anchor_c, row, col));
            self.selected_cell = Some((row, col));
            self.is_dragging = false;
        } else {
            self.selected_range = Some(CellRange::single(row, col));
            self.selected_cell = Some((row, col));
            self.is_dragging = true;
        }
    }

    /// Handles mouse drag over a cell (display coordinates).
    pub fn handle_cell_mouse_drag(&mut self, row: usize, col: usize) {
        if self.is_dragging {
            if let Some(ref mut range) = self.selected_range {
                range.focus_row = row;
                range.focus_col = col;
                self.selected_cell = Some((row, col));
            }
        }
    }

    /// Ends cell dragging.
    pub fn end_drag(&mut self) {
        self.is_dragging = false;
    }

    /// Clears both single cell and range selection.
    pub fn clear_selection_range(&mut self) {
        self.selected_range = None;
        self.selected_cell = None;
        self.is_dragging = false;
    }

    /// Computes summary statistics over all cells within a selection range.
    pub fn compute_selection_stats(&self, range: &CellRange) -> Option<SelectionStats> {
        let total_rows = self.total_row_count();
        if total_rows == 0 || range.cell_count() == 0 {
            return None;
        }
        let min_r = range.min_row().min(total_rows.saturating_sub(1));
        let max_r = range.max_row().min(total_rows.saturating_sub(1));
        let num_cols = self.visible().len();
        if num_cols == 0 {
            return None;
        }
        let min_c = range.min_col().min(num_cols.saturating_sub(1));
        let max_c = range.max_col().min(num_cols.saturating_sub(1));

        let mut values = Vec::with_capacity((max_r - min_r + 1) * (max_c - min_c + 1));
        for r in min_r..=max_r {
            for c in min_c..=max_c {
                let actual_col = self.actual(c);
                let (val_ref, _) = self.cell_value_ref(r, actual_col);
                values.push(val_ref);
            }
        }
        Some(SelectionStats::compute(values.into_iter()))
    }

    /// Formats all cells in the selection range as Tab-Separated Values (TSV).
    pub fn copy_range_tsv(&self, range: &CellRange) -> String {
        let total_rows = self.total_row_count();
        if total_rows == 0 {
            return String::new();
        }
        let min_r = range.min_row().min(total_rows.saturating_sub(1));
        let max_r = range.max_row().min(total_rows.saturating_sub(1));
        let num_cols = self.visible().len();
        if num_cols == 0 {
            return String::new();
        }
        let min_c = range.min_col().min(num_cols.saturating_sub(1));
        let max_c = range.max_col().min(num_cols.saturating_sub(1));

        let mut lines = Vec::new();
        for r in min_r..=max_r {
            let mut line = Vec::new();
            for c in min_c..=max_c {
                let actual_col = self.actual(c);
                let (val_ref, _) = self.cell_value_ref(r, actual_col);
                let s = match val_ref {
                    None | Some(DbValue::Null) => String::new(),
                    Some(v) => v.to_display_string(),
                };
                line.push(s);
            }
            lines.push(line.join("\t"));
        }
        lines.join("\n")
    }

    /// Value of a pending-insert cell as a reference; `None` = left to the column DEFAULT
    fn inserted_value_ref(&self, row: usize, col: usize) -> Option<&DbValue> {
        let name = self.column_name(col)?;
        let insert = self
            .pending
            .row_inserts
            .get(row.checked_sub(self.loaded_len())?)?;
        insert.values.get(name)
    }

    /// The value a cell shows as a reference (pending edits win over loaded values) and whether it was edited
    /// The first non-null loaded value decides, matching how cells align
    fn is_numeric_column(&self, col: usize) -> bool {
        self.result().is_some_and(|r| {
            r.rows
                .iter()
                .filter_map(|row| row.get(col))
                .find(|v| !matches!(v, DbValue::Null))
                .is_some_and(|v| {
                    matches!(v, DbValue::Int(_) | DbValue::Float(_) | DbValue::Decimal(_))
                })
        })
    }

    fn cell_value_ref(&self, row: usize, col: usize) -> (Option<&DbValue>, bool) {
        if row >= self.loaded_len() {
            return (self.inserted_value_ref(row, col), false);
        }
        let Some(original) = self.result().and_then(|r| r.rows.get(row)?.get(col)) else {
            return (None, false);
        };
        // Fast path: if there are no pending cell edits, avoid allocating primary keys and searching edits
        if self.pending.cell_edits.is_empty() {
            return (Some(original), false);
        }
        let edited = self
            .row_key(row)
            .zip(self.column_name(col))
            .and_then(|(k, name)| self.pending.cell_value(&k, name));
        match edited {
            Some(v) => (Some(v), true),
            None => (Some(original), false),
        }
    }

    /// The value a cell shows (pending edits win over loaded values) and whether it was edited
    fn cell_value(&self, row: usize, col: usize) -> (Option<DbValue>, bool) {
        let (val, edited) = self.cell_value_ref(row, col);
        (val.cloned(), edited)
    }

    /// Call before changing `pending`; repeated calls with nothing changed in between are one step
    fn checkpoint(&mut self) {
        if self.undo.last() != Some(&self.pending) {
            self.undo.push(self.pending.clone());
        }
        self.redo.clear();
    }

    /// One step back (`redo == false`) or forward through pending changes
    pub fn step_history(&mut self, redo: bool) {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        // A checkpoint taken by a change that turned out to be a no-op isn't a step
        while from.last() == Some(&self.pending) {
            from.pop();
        }
        if let Some(prev) = from.pop() {
            to.push(std::mem::replace(&mut self.pending, prev));
            self.editing = None;
            self.notice = None;
        }
    }

    /// After a submit: what was pending is now in the database, nothing to step back to
    pub fn forget_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    fn set_value(&mut self, row: usize, col: usize, value: DbValue) {
        self.checkpoint();
        let Some(name) = self.column_name(col).map(str::to_string) else {
            return;
        };
        let loaded = self.loaded_len();
        if row >= loaded {
            if let Some(insert) = self.pending.row_inserts.get_mut(row - loaded) {
                insert.values.insert(name, value);
            }
        } else if let (Some(key), Some(original)) = (
            self.row_key(row),
            self.result()
                .and_then(|r| r.rows.get(row)?.get(col).cloned()),
        ) {
            self.pending.set_cell(key, &name, original, value);
        }
    }
}

/// DataGrip's pager: |< <  1–500 of ?  > >|. The range opens a page-size menu; "?" counts the rows.
fn pager(
    grid: &Entity<TableState<ResultSet>>,
    rs: &ResultSet,
    theme: &Theme,
) -> Option<impl IntoElement> {
    let src = rs.source.as_ref()?;
    let rows = rs.loaded_len();
    let (offset, page, has_more, total) = (src.offset, src.page_size, src.has_more, src.total);
    let busy = src.loading || src.counting;
    // Everything on one page: the arrows would only ever be disabled
    let paged = offset > 0 || has_more;
    let nav = |id: &'static str, icon: IconName, tip: &'static str, enabled: bool, to: PageTo| {
        let grid = grid.clone();
        zbutton(id)
            .icon(icon)
            .ghost()
            .small()
            .tooltip(tip)
            .disabled(!enabled || busy)
            .on_click(move |_, window, cx| {
                grid.update(cx, |t, cx| t.delegate_mut().go_to_page(to, window, cx))
            })
    };
    let range = if rows == 0 {
        "0 rows".to_string()
    } else {
        format!("{}-{}", group(offset + 1), group(offset + rows))
    };
    let sizes = {
        let grid = grid.clone();
        zbutton("grid-page-size")
            .pad_xs()
            .label(range)
            .ghost()
            .small()
            .tooltip("Rows per page")
            .dropdown_menu(move |mut menu, _, _| {
                for n in [100, 250, 500, 1000, 5000] {
                    let grid = grid.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{} rows per page", group(n)))
                            .checked(n == page)
                            .on_click(move |_, window, cx| {
                                grid.update(cx, |t, cx| {
                                    t.delegate_mut().set_page_size(n, window, cx)
                                })
                            }),
                    );
                }
                menu
            })
    };
    let of = {
        let grid = grid.clone();
        let counted = total.is_some();
        div()
            .id("grid-row-count")
            .px_1()
            .rounded(px(crate::ui::RADIUS_SM))
            .text_sm()
            .text_color(if counted {
                theme.text_secondary
            } else {
                theme.accent
            })
            .when(!counted, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover))
                    .on_click(move |_, window, cx| {
                        grid.update(cx, |t, cx| t.delegate_mut().count_rows(false, window, cx))
                    })
            })
            .tooltip(move |window, cx| {
                Tooltip::new(if counted {
                    "Rows matching the filter"
                } else {
                    "Count rows"
                })
                .build(window, cx)
            })
            .child(match total {
                Some(n) => format!("of {}", group(n as usize)),
                None if src.counting => "of …".to_string(),
                None => "of ?".to_string(),
            })
    };
    Some(
        h_flex()
            .items_center()
            .gap_0p5()
            .child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
            .when(paged, |d| {
                d.child(nav(
                    "grid-first",
                    IconName::ChevronsLeft,
                    "First page",
                    offset > 0,
                    PageTo::First,
                ))
            })
            .when(paged, |d| {
                d.child(nav(
                    "grid-prev",
                    IconName::ChevronLeft,
                    "Previous page",
                    offset > 0,
                    PageTo::Prev,
                ))
            })
            .child(sizes)
            .child(of)
            .when(paged, |d| {
                d.child(nav(
                    "grid-next",
                    IconName::ChevronRight,
                    "Next page",
                    has_more,
                    PageTo::Next,
                ))
            })
            .when(paged, |d| {
                d.child(nav(
                    "grid-last",
                    IconName::ChevronsRight,
                    "Last page",
                    has_more,
                    PageTo::Last,
                ))
            }),
    )
}

/// 12345 → "12,345"
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn render_transposed_view<T: 'static>(
    grid: &Entity<TableState<ResultSet>>,
    theme: &Theme,
    cx: &Context<T>,
) -> impl IntoElement {
    let rs = grid.read(cx).delegate();
    let total_rows = rs.total_row_count();
    let mono = cx.theme().mono_font_family.clone();

    if total_rows == 0 {
        let grid_close = grid.clone();
        return v_flex()
            .id("transposed-empty")
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child("No rows in current result set"),
            )
            .child(
                zbutton("transposed-exit-empty")
                    .label("Return to Table")
                    .ghost()
                    .small()
                    .on_click(move |_, _, cx| {
                        grid_close.update(cx, |t, cx| {
                            t.delegate_mut().transposed_view = false;
                            cx.notify();
                        });
                    }),
            )
            .into_any_element();
    }

    let row = rs.transposed_row.min(total_rows.saturating_sub(1));
    let query = rs
        .transposed_search_input
        .as_ref()
        .map(|s| s.read(cx).value().trim().to_lowercase())
        .unwrap_or_default();

    let visible = rs.visible();
    let total_cols = visible.len();

    struct TransposedColItem {
        display_col: usize,
        actual_col: usize,
        name: String,
        type_name: String,
        is_pk: bool,
        val_display: String,
        is_null: bool,
        is_pending: bool,
    }

    let mut items = Vec::new();
    for (display_col, &actual_col) in visible.iter().enumerate() {
        let name = rs.column_name(actual_col).unwrap_or("?").to_string();
        let schema = rs.column_schema(actual_col);
        let type_name = schema
            .map(|s| s.raw_type_name.to_uppercase())
            .unwrap_or_else(|| "ANY".into());
        let is_pk = schema.map(|s| s.is_primary_key).unwrap_or(false);

        if !query.is_empty()
            && !name.to_lowercase().contains(&query)
            && !type_name.to_lowercase().contains(&query)
        {
            continue;
        }

        let (val, is_pending) = rs.cell_value(row, actual_col);
        let is_null = val.as_ref().map_or(true, |v| v.is_null());
        let val_display = match &val {
            Some(v) if !v.is_null() => v.to_display_string(),
            _ => rs.null_label.to_string(),
        };

        items.push(TransposedColItem {
            display_col,
            actual_col,
            name,
            type_name,
            is_pk,
            val_display,
            is_null,
            is_pending,
        });
    }

    let matching_count = items.len();

    let g_prev = grid.clone();
    let g_next = grid.clone();
    let g_close = grid.clone();
    let g_copy_json = grid.clone();
    let g_copy_tsv = grid.clone();
    let null_text = rs.null_label.clone();

    v_flex()
        .id("transposed-view")
        .key_context("TransposedView")
        .size_full()
        .bg(theme.bg_app)
        .on_key_down({
            let g = grid.clone();
            move |ev: &KeyDownEvent, _, cx| {
                let key = ev.keystroke.key.as_str();
                if key == "escape" {
                    g.update(cx, |t, cx| {
                        t.delegate_mut().transposed_view = false;
                        cx.notify();
                    });
                    cx.stop_propagation();
                } else if key == "up" || key == "[" {
                    g.update(cx, |t, cx| {
                        t.delegate_mut().transposed_prev_row();
                        cx.notify();
                    });
                    cx.stop_propagation();
                } else if key == "down" || key == "]" {
                    g.update(cx, |t, cx| {
                        t.delegate_mut().transposed_next_row();
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
            }
        })
        // Top Toolbar
        .child(
            h_flex()
                .h(px(BAR_H))
                .px_3()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.bg_sidebar)
                // Left: Row navigation
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(
                            zbutton("transposed-prev-btn")
                                .icon(IconName::ChevronLeft)
                                .ghost()
                                .xsmall()
                                .disabled(row == 0)
                                .tooltip(crate::ui::format_keys("Previous row (↑ or [)"))
                                .on_click(move |_, _, cx| {
                                    g_prev.update(cx, |t, cx| {
                                        t.delegate_mut().transposed_prev_row();
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(crate::ui::RADIUS_SM))
                                .bg(theme.bg_hover)
                                .font_family(mono.clone())
                                .font_weight(FontWeight::BOLD)
                                .text_sm()
                                .text_color(theme.text_primary)
                                .child(format!("Row {} of {}", row + 1, total_rows)),
                        )
                        .child(
                            zbutton("transposed-next-btn")
                                .icon(IconName::ChevronRight)
                                .ghost()
                                .xsmall()
                                .disabled(row + 1 >= total_rows)
                                .tooltip(crate::ui::format_keys("Next row (↓ or ])"))
                                .on_click(move |_, _, cx| {
                                    g_next.update(cx, |t, cx| {
                                        t.delegate_mut().transposed_next_row();
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                // Center: Column search filter
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .when_some(rs.transposed_search_input.clone(), |this, input| {
                            this.child(
                                div()
                                    .w(px(240.0))
                                    .child(crate::ui::search_input(&input, IconName::ListFilter)),
                            )
                        })
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text_muted)
                                .child(format!("{matching_count} of {total_cols} columns")),
                        ),
                )
                // Right: Copy & Exit
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            zbutton("transposed-copy-json")
                                .pad_xs()
                                .label("Copy JSON")
                                .ghost()
                                .small()
                                .tooltip("Copy row as a JSON object")
                                .on_click(move |_, _, cx| {
                                    let text =
                                        g_copy_json.read(cx).delegate().row_pairs_text(row, true);
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                }),
                        )
                        .child(
                            zbutton("transposed-copy-tsv")
                                .pad_xs()
                                .label("Copy TSV")
                                .ghost()
                                .small()
                                .tooltip("Copy row as column/value lines")
                                .on_click(move |_, _, cx| {
                                    let text =
                                        g_copy_tsv.read(cx).delegate().row_pairs_text(row, false);
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                }),
                        )
                        .child(div().w_px().h(px(16.0)).mx_1().bg(theme.border))
                        .child(
                            zbutton("transposed-close-btn")
                                .icon(IconName::X)
                                .ghost()
                                .small()
                                .tooltip(crate::ui::format_keys("Exit transposed view (⌥R or Esc)"))
                                .on_click(move |_, _, cx| {
                                    g_close.update(cx, |t, cx| {
                                        t.delegate_mut().transposed_view = false;
                                        cx.notify();
                                    });
                                }),
                        ),
                ),
        )
        // Scrollable List of Columns
        .child(
            div()
                .id("transposed-col-list")
                .flex_1()
                .overflow_y_scroll()
                .p_2()
                .children(items.into_iter().map(|item| {
                    let g_inspect = grid.clone();
                    let val_copy = item.val_display.clone();
                    let null_copy = null_text.clone();
                    let display_col = item.display_col;
                    let actual_col = item.actual_col;

                    h_flex()
                        .id(("transposed-col-item", actual_col))
                        .min_h(px(CONTROL_H))
                        .px_3()
                        .py_1p5()
                        .rounded(px(crate::ui::RADIUS_SM))
                        .border_b_1()
                        .border_color(theme.border)
                        // Multi-line values grow down; name and actions stay on the first line
                        .items_start()
                        .hover(|s| s.bg(theme.bg_hover))
                        // Left column: metadata
                        .child(
                            h_flex()
                                .w(px(260.0))
                                .min_w(px(200.0))
                                .flex_shrink_0()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .w(px(28.0))
                                        .text_xs()
                                        .font_family(mono.clone())
                                        .text_color(theme.text_muted)
                                        .child(format!("#{}", display_col + 1)),
                                )
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_sm()
                                        .text_color(theme.text_primary)
                                        .truncate()
                                        .child(item.name),
                                )
                                .when(item.is_pk, |this| {
                                    this.child(
                                        div()
                                            .px_1()
                                            .py(px(0.5))
                                            .rounded(px(crate::ui::RADIUS_SM))
                                            .bg(Theme::tint(theme.warning, 0.15))
                                            .border_1()
                                            .border_color(Theme::tint(theme.warning, 0.4))
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.warning)
                                            .child("PK"),
                                    )
                                })
                                .child(
                                    div()
                                        .px_1p5()
                                        .py(px(0.5))
                                        .rounded(px(crate::ui::RADIUS_SM))
                                        .bg(theme.bg_surface)
                                        .border_1()
                                        .border_color(theme.border)
                                        .text_xs()
                                        .font_family(mono.clone())
                                        .text_color(theme.text_muted)
                                        .child(item.type_name),
                                ),
                        )
                        // Middle: Value
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .px_3()
                                .items_center()
                                .when(item.is_null, |this| {
                                    this.child(
                                        div()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded(px(crate::ui::RADIUS_SM))
                                            .bg(theme.bg_surface)
                                            .border_1()
                                            .border_color(theme.border)
                                            .text_xs()
                                            .italic()
                                            .text_color(theme.text_muted)
                                            .child(null_copy),
                                    )
                                })
                                .when(!item.is_null, |this| {
                                    this.child(
                                        div()
                                            .min_w_0()
                                            .font_family(mono.clone())
                                            .text_sm()
                                            .text_color(theme.text_primary)
                                            .line_clamp(8)
                                            .child(item.val_display),
                                    )
                                })
                                .when(item.is_pending, |this| {
                                    this.child(
                                        div()
                                            .ml_2()
                                            .px_1p5()
                                            .py(px(0.5))
                                            .rounded(px(crate::ui::RADIUS_SM))
                                            .bg(theme.pending_cell_bg)
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.warning)
                                            .child("Modified"),
                                    )
                                }),
                        )
                        // Right: Actions
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .flex_shrink_0()
                                .child(
                                    zbutton(("transposed-copy-val", actual_col))
                                        .icon(IconName::Copy)
                                        .ghost()
                                        .xsmall()
                                        .tooltip("Copy value")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                val_copy.clone(),
                                            ));
                                        }),
                                )
                                .child(
                                    zbutton(("transposed-open-val", actual_col))
                                        .icon(IconName::PanelRight)
                                        .ghost()
                                        .xsmall()
                                        .tooltip("Inspect in Value Editor")
                                        .on_click(move |_, window, cx| {
                                            g_inspect.update(cx, |t, cx| {
                                                t.delegate_mut().inspect(
                                                    row,
                                                    display_col,
                                                    window,
                                                    cx,
                                                );
                                                cx.notify();
                                            });
                                        }),
                                ),
                        )
                })),
        )
        .into_any_element()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JsonValType {
    Object,
    Array,
    String,
    Number,
    Bool(bool),
    Null,
}

struct JsonFlatItem {
    path: String,
    depth: usize,
    key: Option<String>,
    summary: Option<String>,
    val_display: Option<String>,
    val_type: JsonValType,
    is_container: bool,
    is_expanded: bool,
}

fn matches_json_search(val: &serde_json::Value, key: Option<&str>, q: &str) -> bool {
    if let Some(k) = key {
        if k.to_lowercase().contains(q) {
            return true;
        }
    }
    match val {
        serde_json::Value::String(s) => s.to_lowercase().contains(q),
        serde_json::Value::Number(n) => n.to_string().contains(q),
        serde_json::Value::Bool(b) => b.to_string().contains(q),
        serde_json::Value::Null => "null".contains(q),
        serde_json::Value::Object(map) => {
            map.iter().any(|(k, v)| matches_json_search(v, Some(k), q))
        }
        serde_json::Value::Array(arr) => arr.iter().any(|v| matches_json_search(v, None, q)),
    }
}

/// One-line preview of a string leaf: long prompts and blobs are laid out in full on every
/// frame otherwise, though only the first few dozen characters are visible
fn preview(s: &str) -> String {
    const MAX: usize = 160;
    let mut out: String = s
        .chars()
        .take(MAX)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if s.chars().nth(MAX).is_some() {
        out.push('…');
    }
    format!("\"{out}\"")
}

/// The value at a `$.a[0].b` path (as built by `flatten_json_tree`)
fn json_at<'a>(
    cur: &'a serde_json::Value,
    target: &str,
    path: &str,
) -> Option<&'a serde_json::Value> {
    if target == path {
        return Some(cur);
    }
    if !target.starts_with(path) {
        return None;
    }
    match cur {
        serde_json::Value::Object(map) => map.iter().find_map(|(k, v)| {
            let next = if path == "$" {
                format!("$.{k}")
            } else {
                format!("{path}.{k}")
            };
            json_at(v, target, &next)
        }),
        serde_json::Value::Array(arr) => arr
            .iter()
            .enumerate()
            .find_map(|(i, v)| json_at(v, target, &format!("{path}[{i}]"))),
        _ => None,
    }
}

fn flatten_json_tree(
    val: &serde_json::Value,
    key: Option<String>,
    path: String,
    depth: usize,
    expanded: &std::collections::HashSet<String>,
    query: &str,
    out: &mut Vec<JsonFlatItem>,
) {
    if !query.is_empty() && !matches_json_search(val, key.as_deref(), query) {
        return;
    }

    let is_expanded = if !query.is_empty() {
        true
    } else {
        expanded.contains(&path)
    };

    match val {
        serde_json::Value::Object(map) => {
            let count = map.len();
            let summary = format!(
                "{{ {} }}",
                if count == 1 {
                    "1 key".to_string()
                } else {
                    format!("{count} keys")
                }
            );
            out.push(JsonFlatItem {
                path: path.clone(),
                depth,
                key,
                summary: Some(summary),
                val_display: None,
                val_type: JsonValType::Object,
                is_container: true,
                is_expanded,
            });
            if is_expanded {
                for (k, v) in map {
                    let child_path = if path == "$" {
                        format!("$.{k}")
                    } else {
                        format!("{path}.{k}")
                    };
                    flatten_json_tree(
                        v,
                        Some(k.clone()),
                        child_path,
                        depth + 1,
                        expanded,
                        query,
                        out,
                    );
                }
            }
        }
        serde_json::Value::Array(arr) => {
            let count = arr.len();
            let summary = format!(
                "[ {} ]",
                if count == 1 {
                    "1 item".to_string()
                } else {
                    format!("{count} items")
                }
            );
            out.push(JsonFlatItem {
                path: path.clone(),
                depth,
                key,
                summary: Some(summary),
                val_display: None,
                val_type: JsonValType::Array,
                is_container: true,
                is_expanded,
            });
            if is_expanded {
                for (i, v) in arr.iter().enumerate() {
                    let child_path = format!("{path}[{i}]");
                    flatten_json_tree(
                        v,
                        Some(format!("[{i}]")),
                        child_path,
                        depth + 1,
                        expanded,
                        query,
                        out,
                    );
                }
            }
        }
        serde_json::Value::String(s) => {
            out.push(JsonFlatItem {
                path,
                depth,
                key,
                summary: None,
                val_display: Some(preview(s)),
                val_type: JsonValType::String,
                is_container: false,
                is_expanded: false,
            });
        }
        serde_json::Value::Number(n) => {
            out.push(JsonFlatItem {
                path,
                depth,
                key,
                summary: None,
                val_display: Some(n.to_string()),
                val_type: JsonValType::Number,
                is_container: false,
                is_expanded: false,
            });
        }
        serde_json::Value::Bool(b) => {
            out.push(JsonFlatItem {
                path,
                depth,
                key,
                summary: None,
                val_display: Some(b.to_string()),
                val_type: JsonValType::Bool(*b),
                is_container: false,
                is_expanded: false,
            });
        }
        serde_json::Value::Null => {
            out.push(JsonFlatItem {
                path,
                depth,
                key,
                summary: None,
                val_display: Some("null".to_string()),
                val_type: JsonValType::Null,
                is_container: false,
                is_expanded: false,
            });
        }
    }
}

fn render_json_tree<T: 'static>(
    grid: &Entity<TableState<ResultSet>>,
    theme: &Theme,
    cx: &Context<T>,
    parsed_json: &std::rc::Rc<serde_json::Value>,
    expanded_paths: &std::collections::HashSet<String>,
    search_input: Option<&Entity<InputState>>,
    tree_edit: Option<(String, Entity<InputState>)>,
) -> impl IntoElement {
    let mono = cx.theme().mono_font_family.clone();
    let query = search_input
        .map(|s| s.read(cx).value().trim().to_lowercase())
        .unwrap_or_default();

    let mut items = Vec::new();
    flatten_json_tree(
        parsed_json,
        None,
        "$".to_string(),
        0,
        expanded_paths,
        &query,
        &mut items,
    );

    let g_tree = grid.clone();

    div()
        .id("json-tree-container")
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .children(items.into_iter().enumerate().map(|(ix, item)| {
            let g = g_tree.clone();
            let path = item.path.clone();
            let (root_copy, root_edit) = (parsed_json.clone(), parsed_json.clone());
            let depth_indent = px((item.depth as f32) * 14.0);

            let editing = tree_edit
                .as_ref()
                .filter(|(p, _)| *p == item.path)
                .map(|(_, input)| input.clone());

            h_flex()
                .id(("json-tree-row", ix))
                .group("json-row")
                .min_h(px(ROW_H))
                .pl(depth_indent)
                .pr_2()
                .py(px(1.5))
                .rounded(px(crate::ui::RADIUS_SM))
                .items_center()
                .justify_between()
                .hover(|s| s.bg(theme.bg_hover))
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .min_w_0()
                        .flex_1()
                        .when(item.is_container, |this| {
                            let g_toggle = g.clone();
                            let p = path.clone();
                            this.child(
                                zbutton(("json-toggle", ix))
                                    .icon(if item.is_expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .ghost()
                                    .xsmall()
                                    .on_click(move |_, _, cx| {
                                        g_toggle.update(cx, |t, cx| {
                                            t.delegate_mut().toggle_json_tree_path(p.clone());
                                            cx.notify();
                                        });
                                    }),
                            )
                        })
                        .when(!item.is_container, |this| this.child(div().w(px(18.0))))
                        .when_some(item.key, |this, k| {
                            this.child(
                                div()
                                    .font_family(mono.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_xs()
                                    .text_color(if item.is_container {
                                        theme.accent
                                    } else {
                                        theme.text_primary
                                    })
                                    .child(k),
                            )
                            .child(div().text_xs().text_color(theme.text_muted).child(":"))
                        })
                        .when_some(item.summary, |this, sum| {
                            this.child(
                                div()
                                    .px_1()
                                    .py(px(0.5))
                                    .rounded(px(crate::ui::RADIUS_SM))
                                    .bg(theme.bg_surface)
                                    .border_1()
                                    .border_color(theme.border)
                                    .text_xs()
                                    .font_family(mono.clone())
                                    .text_color(theme.text_muted)
                                    .child(sum),
                            )
                        })
                        .when_some(item.val_display, |this, val_str| {
                            let (val_color, is_italic) = match item.val_type {
                                JsonValType::String => (Theme::tint(theme.success, 0.95), false),
                                JsonValType::Number => (theme.accent.into(), false),
                                JsonValType::Bool(true) => (theme.success.into(), false),
                                JsonValType::Bool(false) => (theme.danger.into(), false),
                                JsonValType::Null => (theme.text_muted.into(), true),
                                _ => (theme.text_primary.into(), false),
                            };
                            if let Some(input) = editing.clone() {
                                return this
                                    .child(div().flex_1().child(Input::new(&input).xsmall()));
                            }
                            let g_edit = g.clone();
                            let p = path.clone();
                            this.child(
                                div()
                                    .id(("json-val", ix))
                                    .min_w_0()
                                    .font_family(mono.clone())
                                    .text_xs()
                                    .text_color(val_color)
                                    .when(is_italic, |d| d.italic())
                                    .truncate()
                                    .cursor_text()
                                    .child(val_str)
                                    .on_click(move |e, window, cx| {
                                        if e.click_count() >= 2 {
                                            let Some(literal) =
                                                json_at(&root_edit, &p, "$").map(|v| v.to_string())
                                            else {
                                                return;
                                            };
                                            g_edit.update(cx, |t, cx| {
                                                t.delegate_mut().start_tree_edit(
                                                    p.clone(),
                                                    literal,
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }
                                    }),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .invisible()
                        .group_hover("json-row", |s| s.visible())
                        .child(
                            zbutton(("json-copy-val", ix))
                                .icon(IconName::Copy)
                                .ghost()
                                .xsmall()
                                .tooltip("Copy value")
                                .on_click(move |_, _, cx| {
                                    // Strings copy bare; everything else as (pretty) JSON
                                    let text = match json_at(&root_copy, &path, "$") {
                                        Some(serde_json::Value::String(s)) => s.clone(),
                                        Some(v) => {
                                            serde_json::to_string_pretty(v).unwrap_or_default()
                                        }
                                        None => return,
                                    };
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                }),
                        ),
                )
        }))
}

pub struct DataGrid;

impl DataGrid {
    #[allow(clippy::too_many_arguments)]
    pub fn render<T: 'static>(
        grid: &Entity<TableState<ResultSet>>,
        theme: &Theme,
        cx: &Context<T>,
        on_refresh: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_review: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_submit: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_export: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
        on_follow_fk: impl Fn(&mut T, (String, String, String), &mut Window, &mut Context<T>)
            + Copy
            + 'static,
        on_filter: impl Fn(&mut T, String, &mut Window, &mut Context<T>) + Copy + 'static,
        on_open_filter_builder: impl Fn(&mut T, Option<String>, &mut Window, &mut Context<T>)
            + Copy
            + 'static,
    ) -> impl IntoElement {
        let rs = grid.read(cx).delegate();
        let pending_count = rs.pending.total_count();
        let (set_count, active) = (rs.results.len(), rs.active);
        let error = rs.source.as_ref().and_then(|s| s.error.clone());
        let notice = rs.notice.clone();
        let header_menu = rs.header_menu.as_ref().map(|(p, m, _)| (*p, m.clone()));
        let is_table = rs.source.is_some();
        let read_only = rs.read_only_reason();
        let hidden = rs.hidden_count();
        let total_rows = rs.total_row_count();
        let total_cols = rs.visible().len();
        let selected = rs.selected_cell;

        let has_inspector = rs.inspector.is_some();
        let wrap = rs.inspector.as_ref().is_some_and(|i| i.wrap);
        let inspector = rs.inspector.as_ref().map(|i| {
            let col = rs.column_name(i.col).unwrap_or_default().to_string();
            let col_type = rs
                .column_schema(i.col)
                .map(|s| s.raw_type_name.to_uppercase())
                .unwrap_or_else(|| "ANY".into());
            (
                i.editor.clone(),
                col,
                col_type,
                i.row,
                read_only.is_some() || rs.row_state(i.row) == RowState::Deleted,
                i.mode,
                i.parsed_json.clone(),
                i.expanded_paths.clone(),
                i.search_input.clone(),
                i.tree_edit
                    .as_ref()
                    .map(|(p, input, _)| (p.clone(), input.clone())),
            )
        });

        let action_button = |id: &'static str,
                             icon: IconName,
                             tip: &'static str,
                             action: Box<dyn Action>| {
            let grid = grid.clone();
            zbutton(id)
                .icon(icon)
                .ghost()
                .small()
                .tooltip(crate::ui::format_keys(tip))
                .disabled(read_only.is_some())
                .on_click(move |_, window, cx| apply_row_action(&grid, action.as_ref(), window, cx))
        };

        // Toolbar: Zed-quiet icon buttons with DataGrip's operations
        let toolbar = h_flex()
            .h(px(BAR_H))
            .px_3()
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(
                zbutton("grid-refresh")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .small()
                    .tooltip(crate::ui::format_keys("Reload (⌘↵ outside the grid)"))
                    .on_click(cx.listener(move |this, _, window, cx| on_refresh(this, window, cx))),
            )
            .children(pager(grid, rs, theme))
            .child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
            .child(action_button(
                "grid-add",
                IconName::Plus,
                "Add row (⌘N)",
                Box::new(AddRow),
            ))
            .child(action_button(
                "grid-dup",
                IconName::Copy,
                "Duplicate row (⌘D)",
                Box::new(DuplicateRow),
            ))
            .child(action_button(
                "grid-del",
                IconName::Minus,
                "Delete row (⌘⌫)",
                Box::new(DeleteRow),
            ))
            .child(div().w_px().h(px(14.0)).mx_1().bg(theme.border))
            .child({
                let grid = grid.clone();
                zbutton("grid-value-editor")
                    .icon(IconName::PanelRight)
                    .ghost()
                    .small()
                    .tooltip(crate::ui::format_keys("Value editor (⇧↵)"))
                    .on_click(move |_, window, cx| {
                        apply_row_action(&grid, &ToggleValueEditor, window, cx)
                    })
            })
            .child({
                let grid = grid.clone();
                zbutton("grid-transpose")
                    .icon(IconName::Rows2)
                    .ghost()
                    .small()
                    .tooltip(crate::ui::format_keys("Transposed row view (⌥R)"))
                    .when(rs.transposed_view, |b| {
                        b.bg(theme.bg_selected).text_color(theme.accent)
                    })
                    .on_click(move |_, window, cx| {
                        apply_row_action(&grid, &ToggleTransposedView, window, cx);
                    })
            })
            .child(
                zbutton("grid-export")
                    .icon(IconName::Download)
                    .ghost()
                    .small()
                    .tooltip(crate::ui::format_keys("Export (⇧⌘E)"))
                    .on_click(cx.listener(move |this, _, window, cx| on_export(this, window, cx))),
            )
            .when(hidden > 0, |this| {
                let grid = grid.clone();
                this.child(
                    zbutton("grid-hidden")
                        .pad_xs()
                        .label(format!("{hidden} hidden"))
                        .ghost()
                        .small()
                        .tooltip("Show all columns")
                        .on_click(move |_, window, cx| {
                            apply_row_action(&grid, &ShowAllColumns, window, cx)
                        }),
                )
            })
            .child(div().flex_1())
            .when_some(notice.clone(), |this, n| {
                this.child(
                    div()
                        .px_2()
                        .text_sm()
                        .text_color(theme.danger)
                        .truncate()
                        .child(n),
                )
            })
            .when_some(read_only.filter(|_| notice.is_none()), |this, reason| {
                this.child(
                    div()
                        .px_2()
                        .text_sm()
                        .text_color(theme.text_muted)
                        .child(reason),
                )
            })
            .when(pending_count > 0, |this| {
                let grid = grid.clone();
                this.child(
                    div()
                        .px_1()
                        .text_sm()
                        .text_color(theme.warning)
                        .child(format!(
                            "{} pending {}",
                            pending_count,
                            if pending_count == 1 {
                                "change"
                            } else {
                                "changes"
                            }
                        )),
                )
                .child(
                    zbutton("grid-revert")
                        .icon(IconName::Undo2)
                        .ghost()
                        .small()
                        .tooltip(crate::ui::format_keys("Revert all (⌥⌘Z)"))
                        .on_click(move |_, _, cx| {
                            grid.update(cx, |t, cx| {
                                t.delegate_mut().revert();
                                t.refresh(cx);
                                cx.notify();
                            })
                        }),
                )
                .child(
                    zbutton("grid-review")
                        .ml_1p5()
                        .pad_xxs()
                        .label("Review")
                        .ghost()
                        .small()
                        .tooltip(crate::ui::format_keys("Show the SQL (⌘S)"))
                        .on_click(
                            cx.listener(move |this, _, window, cx| on_review(this, window, cx)),
                        ),
                )
                .child(
                    zbutton("grid-submit")
                        .ml_1p5()
                        .pad_xxs()
                        .icon(IconName::Check)
                        .label("Submit")
                        .primary()
                        .small()
                        .tooltip(crate::ui::format_keys("Apply in one transaction (⌘↵)"))
                        .on_click(
                            cx.listener(move |this, _, window, cx| on_submit(this, window, cx)),
                        ),
                )
            });

        let grid_cancel = grid.clone();
        let grid_revert = grid.clone();
        let grid_out = grid.clone();

        v_flex()
            .id("data-grid-root")
            .size_full()
            .bg(theme.bg_app)
            .on_mouse_down_out(move |_, _, cx| {
                grid_out.update(cx, |t, cx| {
                    t.delegate_mut().finish_edit(true, cx);
                    t.clear_selection(cx);
                    t.delegate_mut().selected_cell = None;
                    t.refresh(cx);
                });
            })
            .when(is_table, |this| this.child(toolbar))
            .when_some(header_menu, |this, (pos, menu)| {
                this.child(
                    deferred(anchored().position(pos).snap_to_window().child(menu))
                        .with_priority(1),
                )
            })
            .when(!is_table, |this| {
                this.when_some(notice.clone(), |this, n| {
                    this.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .border_b_1()
                            .border_color(theme.border)
                            .child(n),
                    )
                })
            })
            // Result sub-tabs, DataGrip style: one per row-returning statement
            .when(set_count > 1, |this| {
                this.child(
                    h_flex()
                        .h(px(CONTROL_H))
                        .px_2()
                        .gap_1()
                        .items_center()
                        .border_b_1()
                        .border_color(theme.border)
                        .children((0..set_count).map(|ix| {
                            let grid = grid.clone();
                            div()
                                .id(("result-set", ix))
                                .px_2()
                                .py_0p5()
                                .rounded(px(crate::ui::RADIUS_SM))
                                .text_sm()
                                .cursor_pointer()
                                .when(ix == active, |d| {
                                    d.bg(theme.bg_selected).text_color(theme.text_primary)
                                })
                                .when(ix != active, |d| {
                                    d.text_color(theme.text_muted)
                                        .hover(|s| s.bg(theme.bg_hover))
                                })
                                .on_click(move |_, _, cx| {
                                    grid.update(cx, |t, cx| {
                                        t.delegate_mut().set_active(ix);
                                        t.refresh(cx);
                                        cx.notify();
                                    });
                                })
                                .child(format!("Result {}", ix + 1))
                        })),
                )
            })
            .when_some(error, |this, err| {
                this.child(
                    div()
                        .px_3()
                        .py_1p5()
                        .text_sm()
                        .text_color(theme.danger)
                        .border_b_1()
                        .border_color(theme.border)
                        .child(err),
                )
            })
            .on_action({
                let g = grid.clone();
                move |a: &EditCell, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &AddRow, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &DuplicateRow, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &DeleteRow, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &SetNull, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &SetDefault, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &ToggleValueEditor, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyValue, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyRowTsv, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyRowCsv, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyRowJson, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyRowInsert, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyAllTsv, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyAllCsv, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyAllJson, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &CopyAllInsert, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &HideColumn, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &ShowAllColumns, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                cx.listener(move |this, _: &FollowForeignKey, window, cx| {
                    let target = g
                        .read(cx)
                        .selected_cell()
                        .and_then(|(row, col)| g.read(cx).delegate().foreign_key_target(row, col));
                    if let Some(target) = target {
                        on_follow_fk(this, target, window, cx);
                    }
                })
            })
            .on_action({
                let g = grid.clone();
                cx.listener(move |this, _: &FilterByValue, window, cx| {
                    if let Some(cond) = g.read(cx).delegate().filter_by_selected_cell(true) {
                        on_filter(this, cond, window, cx);
                    }
                })
            })
            .on_action({
                let g = grid.clone();
                cx.listener(move |this, _: &FilterExcludeValue, window, cx| {
                    if let Some(cond) = g.read(cx).delegate().filter_by_selected_cell(false) {
                        on_filter(this, cond, window, cx);
                    }
                })
            })
            .on_action({
                let g = grid.clone();
                cx.listener(move |this, _: &ApplyPendingFilter, window, cx| {
                    let clause = g.update(cx, |t, _| t.delegate_mut().pending_filter_clause.take());
                    if let Some(clause) = clause {
                        on_filter(this, clause, window, cx);
                    }
                })
            })
            .on_action({
                let g = grid.clone();
                cx.listener(move |this, _: &OpenFilterBuilder, window, cx| {
                    let col = g.update(cx, |t, _| t.delegate_mut().pending_filter_builder_col.take());
                    on_open_filter_builder(this, col, window, cx);
                })
            })
            .on_action({
                let g = grid.clone();
                move |_: &NextCell, window, cx| next_cell(&g, window, cx)
            })
            .on_action({
                let g = grid.clone();
                move |_: &PrevCell, window, cx| prev_cell(&g, window, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &SelectRangeUp, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &SelectRangeDown, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &SelectRangeLeft, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &SelectRangeRight, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &ToggleTransposedView, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &TransposedNextRow, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action({
                let g = grid.clone();
                move |a: &TransposedPrevRow, w, cx| apply_row_action(&g, a, w, cx)
            })
            .on_action(move |_: &CancelEdit, window, cx| {
                grid_cancel.update(cx, |t, cx| {
                    t.delegate_mut().finish_edit(false, cx);
                    window.focus(&t.focus_handle(cx), cx);
                })
            })
            .on_action({
                let g = grid.clone();
                move |_: &UndoChange, _, cx| {
                    g.update(cx, |t, cx| {
                        t.delegate_mut().step_history(false);
                        t.refresh(cx);
                        cx.notify();
                    })
                }
            })
            .on_action({
                let g = grid.clone();
                move |_: &RedoChange, _, cx| {
                    g.update(cx, |t, cx| {
                        t.delegate_mut().step_history(true);
                        t.refresh(cx);
                        cx.notify();
                    })
                }
            })
            .on_action(move |_: &RevertChanges, _, cx| {
                grid_revert.update(cx, |t, cx| {
                    t.delegate_mut().revert();
                    t.refresh(cx);
                    cx.notify();
                })
            })
            .on_action(cx.listener({
                let g = grid.clone();
                move |this, _: &SubmitChanges, window, cx| {
                    g.update(cx, |t, cx| t.delegate_mut().finish_edit(true, cx));
                    on_submit(this, window, cx);
                }
            }))
            // Type-to-edit: a printable key on a selected cell starts editing with that character (DataGrip)
            .on_key_down({
                let grid = grid.clone();
                move |ev: &KeyDownEvent, window, cx| {
                    let ks = &ev.keystroke;
                    if ks.modifiers.platform || ks.modifiers.control || ks.modifiers.function {
                        return;
                    }
                    let Some(ch) = ks
                        .key_char
                        .clone()
                        .filter(|c| !c.chars().any(char::is_control))
                    else {
                        return;
                    };
                    grid.update(cx, |t, cx| {
                        // Keys typed in the value editor or a filter bubble up here too
                        if !t.focus_handle(cx).is_focused(window) {
                            return;
                        }
                        let Some((row, col)) = t.selected_cell() else {
                            return;
                        };
                        if t.delegate().editing.is_none() {
                            t.delegate_mut().start_edit(row, col, Some(ch), window, cx);
                            cx.stop_propagation();
                        }
                    });
                }
            })
            // Same split as the sidebar and bottom panel: one drag feel across the app
            .child(
                h_resizable("grid-value-editor")
                    .on_resize(crate::layout::remember("grid-value-editor"))
                    .child(resizable_panel().child(
                        div()
                            .id("data-table-container")
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .when(!rs.transposed_view, |this| {
                                this.on_mouse_up(MouseButton::Left, {
                                    let g = grid.clone();
                                    move |_, _, cx| {
                                        g.update(cx, |t, cx| {
                                            if t.delegate().is_dragging {
                                                t.delegate_mut().end_drag();
                                                t.refresh(cx);
                                            }
                                        });
                                    }
                                })
                                .on_click({
                                    let g = grid.clone();
                                    move |_, _, cx| {
                                        g.update(cx, |t, cx| {
                                            t.clear_selection(cx);
                                            t.delegate_mut().clear_selection_range();
                                            t.refresh(cx);
                                        });
                                    }
                                })
                                .child(
                                    // Data-tool density: 26px rows, cell padding unchanged. A custom
                                    // size drops the preset's text size, so set it on a wrapper
                                    div().size_full().text_sm().child(
                                        DataTable::new(grid)
                                            .bordered(false)
                                            .with_size(gpui::component::Size::Size(px(26.0))),
                                    ),
                                )
                            })
                            .when(rs.transposed_view, |this| {
                                this.child(render_transposed_view(grid, theme, cx))
                            }),
                    ))
                    .child(
                        resizable_panel()
                            .size(crate::layout::size("grid-value-editor", 1, 460.0))
                            .size_range(px(280.0)..px(1600.0))
                            .visible(has_inspector)
                            .child(div().size_full().when_some(inspector, |this, (editor, col_name, col_type, row, read_only, mode, parsed_json, expanded_paths, search_input, tree_edit)| {
                        let (g_close, g_apply, g_key) = (grid.clone(), grid.clone(), grid.clone());
                        let has_json = parsed_json.is_some();
                        let mono = cx.theme().mono_font_family.clone();
                        this.child(
                            v_flex()
                                .key_context("ValueEditor")
                                .on_action(move |_: &ApplyValue, _, cx| {
                                    g_key.update(cx, |t, cx| t.delegate_mut().apply_inspector(cx))
                                })
                                .size_full()
                                .border_l_1()
                                .border_color(theme.border)
                                .bg(theme.bg_panel)
                                // Top Header
                                .child(
                                    h_flex()
                                        .h(px(BAR_H))
                                        .px_3()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(theme.border)
                                        .bg(theme.bg_sidebar)
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .text_sm()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(theme.text_primary)
                                                        .child(col_name),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.text_muted)
                                                        .child(format!("row {}", row + 1)),
                                                )
                                                .child(
                                                    div()
                                                        .px_1p5()
                                                        .py_0p5()
                                                        .rounded(px(crate::ui::RADIUS_SM))
                                                        .bg(theme.bg_surface)
                                                        .border_1()
                                                        .border_color(theme.border)
                                                        .text_xs()
                                                        .font_family(mono.clone())
                                                        .text_color(theme.text_muted)
                                                        .child(col_type.to_lowercase()),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .items_center()
                                                .when(has_json, |this| {
                                                    let g_tree_mode = grid.clone();
                                                    let g_text_mode = grid.clone();
                                                    // Plain text toggle: the active view is just brighter
                                                    let mode_tab = |id: &'static str, label: &'static str, m: InspectorMode, g: Entity<TableState<ResultSet>>| {
                                                        let on = mode == m;
                                                        div()
                                                            .id(id)
                                                            .text_xs()
                                                            .cursor_pointer()
                                                            .when(on, |d| d.text_color(theme.text_primary).font_weight(FontWeight::SEMIBOLD))
                                                            .when(!on, |d| d.text_color(theme.text_muted).hover(|s| s.text_color(theme.text_secondary)))
                                                            .on_click(move |_, window, cx| {
                                                                g.update(cx, |t, cx| {
                                                                    t.delegate_mut().set_inspector_mode(m, window, cx);
                                                                    cx.notify();
                                                                });
                                                            })
                                                            .child(label)
                                                    };
                                                    this.child(
                                                        h_flex()
                                                            .gap_3()
                                                            .mr_1()
                                                            .child(mode_tab("val-mode-tree", "Tree", InspectorMode::Tree, g_tree_mode))
                                                            .child(mode_tab("val-mode-text", "Text", InspectorMode::Text, g_text_mode)),
                                                    )
                                                })
                                                .child(div().w_px().h(px(14.0)).mx_0p5().bg(theme.border))
                                                .child(
                                                    zbutton("value-editor-close")
                                                        .icon(IconName::X)
                                                        .ghost()
                                                        .xsmall()
                                                        .tooltip("Close Value Editor (Esc)")
                                                        .on_click(move |_, _, cx| {
                                                            g_close.update(cx, |t, cx| {
                                                                t.delegate_mut().close_inspector();
                                                                cx.notify();
                                                            })
                                                        }),
                                                ),
                                        ),
                                )
                                // Secondary Toolbar for Tree Mode
                                .when(mode == InspectorMode::Tree && has_json, |this| {
                                    let g_exp = grid.clone();
                                    let g_col = grid.clone();
                                    this.child(
                                        h_flex()
                                            .h(px(BAR_H))
                                            .px_3()
                                            .items_center()
                                            .justify_between()
                                            .border_b_1()
                                            .border_color(theme.border)
                                            .bg(theme.bg_surface)
                                            .when_some(search_input.clone(), |this, input| {
                                                this.child(
                                                    div().w(px(200.0)).child(
                                                        crate::ui::search_input(&input, IconName::Search),
                                                    ),
                                                )
                                            })
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .child(
                                                        zbutton("val-expand-all")
                                                            .pad_xxs()
                                                            .label("Expand")
                                                            .ghost()
                                                            .xsmall()
                                                            .tooltip("Expand all nodes")
                                                            .on_click(move |_, _, cx| {
                                                                g_exp.update(cx, |t, cx| {
                                                                    t.delegate_mut().expand_all_json_tree();
                                                                    cx.notify();
                                                                });
                                                            }),
                                                    )
                                                    .child(
                                                        zbutton("val-collapse-all")
                                                            .pad_xxs()
                                                            .label("Collapse")
                                                            .ghost()
                                                            .xsmall()
                                                            .tooltip("Collapse all nodes")
                                                            .on_click(move |_, _, cx| {
                                                                g_col.update(cx, |t, cx| {
                                                                    t.delegate_mut().collapse_all_json_tree();
                                                                    cx.notify();
                                                                });
                                                            }),
                                                    )
                                                    ),
                                    )
                                })
                                // Secondary Toolbar for Text Mode when JSON
                                .when(mode == InspectorMode::Text || !has_json, |this| {
                                    let g_fmt = grid.clone();
                                    let g_min = grid.clone();
                                    let g_cp = grid.clone();
                                    this.child(
                                        h_flex()
                                            .h(px(BAR_H))
                                            .px_3()
                                            .items_center()
                                            .justify_end()
                                            .gap_1()
                                            .border_b_1()
                                            .border_color(theme.border)
                                            .bg(theme.bg_surface)
                                            .child({
                                                let g_wrap = grid.clone();
                                                zbutton("val-wrap")
                                                    .pad_xxs()
                                                    .label("Wrap")
                                                    .ghost()
                                                    .xsmall()
                                                    .when(wrap, |b| b.bg(theme.bg_selected).text_color(theme.accent))
                                                    .tooltip("Wrap long lines (slower to resize with very long values)")
                                                    .on_click(move |_, window, cx| {
                                                        g_wrap.update(cx, |t, cx| {
                                                            t.delegate_mut().toggle_inspector_wrap(window, cx);
                                                            cx.notify();
                                                        });
                                                    })
                                            })
                                            // Format / Minify only make sense for JSON
                                            .when(has_json, |d| d
                                            .child(div().w_px().h(px(14.0)).mx_0p5().bg(theme.border))
                                            .child(
                                                zbutton("val-format-json")
                                                    .pad_xxs()
                                                    .label("Format")
                                                    .ghost()
                                                    .xsmall()
                                                    .tooltip("Format JSON (pretty-print)")
                                                    .on_click(move |_, window, cx| {
                                                        g_fmt.update(cx, |t, cx| {
                                                            t.delegate_mut().format_inspector_json(window, cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                            )
                                            .child(
                                                zbutton("val-minify-json")
                                                    .pad_xxs()
                                                    .label("Minify")
                                                    .ghost()
                                                    .xsmall()
                                                    .tooltip("Minify JSON (single-line)")
                                                    .on_click(move |_, window, cx| {
                                                        g_min.update(cx, |t, cx| {
                                                            t.delegate_mut().minify_inspector_json(window, cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                            ))
                                            .child(div().w_px().h(px(14.0)).mx_0p5().bg(theme.border))
                                            .child(
                                                zbutton("val-copy-text")
                                                    .pad_xxs()
                                                    .icon(IconName::Copy)
                                                    .label("Copy")
                                                    .ghost()
                                                    .xsmall()
                                                    .tooltip("Copy text")
                                                    .on_click(move |_, _, cx| {
                                                        g_cp.update(cx, |t, cx| {
                                                            if let Some(ins) = t.delegate().inspector.as_ref() {
                                                                let text = ins.editor.read(cx).value().to_string();
                                                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                                            }
                                                        });
                                                    }),
                                            ),
                                    )
                                })
                                // Body
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .when(mode == InspectorMode::Tree && has_json, |this| {
                                            if let Some(val) = parsed_json.as_ref() {
                                                this.child(render_json_tree(grid, theme, cx, val, &expanded_paths, search_input.as_ref(), tree_edit.clone()))
                                            } else {
                                                this.child(Editor::new(&editor).h_full().bordered(false))
                                            }
                                        })
                                        .when(mode == InspectorMode::Text || !has_json, |this| {
                                            this.child(Editor::new(&editor).h_full().bordered(false))
                                        }),
                                )
                                .when(!read_only, |this| {
                                    this.child(
                                        h_flex()
                                            .h(px(BAR_H))
                                            .px_3()
                                            .items_center()
                                            .justify_between()
                                            .border_t_1()
                                            .border_color(theme.border)
                                            .bg(theme.bg_sidebar)
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.text_muted)
                                                    .child(if mode == InspectorMode::Tree {
                                                        "Double-click a value to edit it"
                                                    } else {
                                                        "⌘↵ to apply"
                                                    }),
                                            )
                                            .child(
                                                zbutton("value-editor-apply")
                                                    .pad_xxs()
                                                    .icon(IconName::Check)
                                                    .label("Apply Changes")
                                                    .primary()
                                                    .xsmall()
                                                    .tooltip(crate::ui::format_keys(
                                                        "Write to the cell (⌘↵)",
                                                    ))
                                                    .on_click(move |_, _, cx| {
                                                        g_apply.update(cx, |t, cx| {
                                                            t.delegate_mut().apply_inspector(cx)
                                                        })
                                                    }),
                                            ),
                                    )
                                }),
                        )
                    }))),
            )
            .child({
                let (range_info, active_col_info, stat_chips) = if let Some(range) = rs.selected_range.as_ref().filter(|r| !r.is_single_cell()) {
                    let sel_stats = rs.compute_selection_stats(range);
                    let chips = sel_stats.map(|s| s.chips()).unwrap_or_default();
                    let is_single_column = range.min_col() == range.max_col();
                    let (info_text, col_info) = if is_single_column {
                        let actual_col = rs.actual(range.min_col());
                        let col_name = rs.column_name(actual_col).unwrap_or("?").to_string();
                        let col_type = rs
                            .column_schema(actual_col)
                            .map(|s| s.raw_type_name.to_uppercase())
                            .unwrap_or_else(|| "ANY".into());
                        (
                            format!(
                                "{} rows ({}-{}) in {}",
                                range.row_count(),
                                range.min_row() + 1,
                                range.max_row() + 1,
                                col_name
                            ),
                            Some((col_name, col_type)),
                        )
                    } else {
                        (
                            format!(
                                "{} rows × {} cols ({} cells)",
                                range.row_count(),
                                range.col_count(),
                                range.cell_count()
                            ),
                            None,
                        )
                    };
                    (info_text, col_info, chips)
                } else if let Some((r, c)) = selected {
                    let actual_col = rs.actual(c);
                    let col_name = rs.column_name(actual_col).unwrap_or("?").to_string();
                    let col_type = rs
                        .column_schema(actual_col)
                        .map(|s| s.raw_type_name.to_uppercase())
                        .unwrap_or_else(|| "ANY".into());
                    let chips = rs
                        .compute_column_stats(actual_col)
                        .map(|s| s.chips())
                        .unwrap_or_default();
                    (
                        format!("Row {} of {} · Col {} of {}", r + 1, total_rows, c + 1, total_cols),
                        Some((col_name, col_type)),
                        chips,
                    )
                } else {
                    (
                        format!("{} rows · {} columns", total_rows, total_cols),
                        None,
                        Vec::new(),
                    )
                };

                let mono = cx.theme().mono_font_family.clone();

                h_flex()
                    .h(px(ROW_H))
                    .w_full()
                    .px_3()
                    .bg(theme.bg_sidebar)
                    .border_t_1()
                    .border_color(theme.border)
                    .gap_3()
                    .items_center()
                    .text_sm()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .flex_shrink_0()
                            .when_some(active_col_info, |this, (name, ty)| {
                                this.child(
                                    div()
                                        .text_color(theme.text_secondary)
                                        .child(format!("{name} · {}", ty.to_lowercase())),
                                )
                            })
                            .child(div().text_color(theme.text_muted).child(range_info)),
                    )
                    .child(
                        // Stats hug the right so their edge never moves as the left text changes
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .justify_end()
                            .items_center()
                            .overflow_hidden()
                            .children(stat_chips.into_iter().enumerate().flat_map(|(ix, chip)| {
                                let sep = (ix > 0).then(|| div().text_color(theme.text_muted).child("·"));
                                let raw = chip.raw_value.clone();
                                let raw_copy = chip.raw_value.clone();
                                let label = chip.label;
                                let chip_el = div()
                                    .id(ElementId::NamedInteger("bottom-bar-stat-chip".into(), ix as u64))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .px_1p5()
                                    .rounded(px(crate::ui::RADIUS_SM))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.bg_hover))
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(format!("Copy {} ({})", label.to_lowercase(), raw)).build(window, cx)
                                    })
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(raw_copy.clone()));
                                    })
                                    .child(div().text_color(theme.text_muted).child(chip.label))
                                    .child(
                                        div()
                                            .font_family(mono.clone())
                                            .text_color(theme.text_primary)
                                            .child(chip.display_value),
                                    );
                                sep.map(|d| d.into_any_element()).into_iter().chain([chip_el.into_any_element()])
                            })),
                    )
                    .child(div().w_px().h(px(14.0)).mx_2().flex_shrink_0().bg(theme.border))
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .flex_shrink_0()
                            .child({
                                let grid = grid.clone();
                                zbutton("footer-transpose")
                                    .label(if rs.transposed_view {
                                        "Exit transpose"
                                    } else {
                                        "Transpose"
                                    })
                                    .ghost()
                                    .xsmall()
                                    .tooltip(crate::ui::format_keys("Transposed row view (⌥R)"))
                                    .when(rs.transposed_view, |b| {
                                        b.bg(theme.bg_selected).text_color(theme.accent)
                                    })
                                    .on_click(move |_, window, cx| {
                                        apply_row_action(&grid, &ToggleTransposedView, window, cx);
                                    })
                            })
                            .child({
                                let grid = grid.clone();
                                zbutton("footer-value-editor")
                                    .label("Value editor")
                                    .ghost()
                                    .xsmall()
                                    .tooltip(crate::ui::format_keys("Value editor (⇧↵)"))
                                    .on_click(move |_, window, cx| {
                                        apply_row_action(&grid, &ToggleValueEditor, window, cx)
                                    })
                            }),
                    )
            })
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` attribute would shadow the std one
    use super::{CellRange, PendingRowInsert, ResultSet, Theme};

    #[test]
    fn date_part_reads_the_leading_date() {
        let d = |s| super::editing::date_part(s).map(|d| d.to_string());
        assert_eq!(
            d("2026-09-18 13:54:48.480843+00").as_deref(),
            Some("2026-09-18")
        );
        assert_eq!(d("2026-02-30"), None);
        assert_eq!(d("soon"), None);
    }

    #[test]
    fn step_segment_moves_the_part_under_the_caret() {
        let step =
            |s, caret, d| super::editing::step_segment(s, caret, d).map(|(t, r)| (t, r.start));
        let ts = "2026-01-31 23:59:58.480843+00";
        assert_eq!(
            step(ts, 6, 1),
            Some(("2026-02-28 23:59:58.480843+00".into(), 5))
        );
        assert_eq!(
            step(ts, 15, 1),
            Some(("2026-02-01 00:00:58.480843+00".into(), 14))
        );
        assert_eq!(
            step(ts, 2, -1),
            Some(("2025-01-31 23:59:58.480843+00".into(), 0))
        );
        assert_eq!(
            step(ts, 25, 3),
            Some(("2026-02-01 00:00:01.480843+00".into(), 17))
        );
        assert_eq!(step("2026-03-01", 9, -1), Some(("2026-02-28".into(), 8)));
        assert_eq!(step("2026-03-01", 30, 1), Some(("2026-03-02".into(), 8)));
        assert_eq!(step("tomorrow", 2, 1), None);
    }

    #[test]
    fn date_readout_spells_it_out() {
        let r = super::editing::date_readout;
        assert_eq!(
            r("2026-09-18 13:54:48.480843+00"),
            ("Fri, 18 Sep 2026 · 13:54:48.480843+00".into(), true)
        );
        assert_eq!(r("2026-09-18"), ("Fri, 18 Sep 2026".into(), true));
        assert!(!r("2026-13-01").1);
        assert!(!r("2026-09-18 25:00").1);
    }

    #[test]
    fn json_tree_lookup_and_preview() {
        use super::{json_at, preview};
        let v = serde_json::json!({"a": [1, {"b": "x"}], "ab": 2});
        assert_eq!(json_at(&v, "$.a[1].b", "$"), Some(&serde_json::json!("x")));
        assert_eq!(json_at(&v, "$.ab", "$"), Some(&serde_json::json!(2)));
        assert_eq!(json_at(&v, "$.zz", "$"), None);
        let long = "line one\r\n".repeat(50);
        let p = preview(&long);
        assert!(p.ends_with("…\"") && !p.contains('\n'));
    }

    #[test]
    fn paging_helpers() {
        assert_eq!(super::group(1234567), "1,234,567");
        assert_eq!(super::group(999), "999");
        assert_eq!(super::editing::last_page_offset(1001, 500), 1000);
        assert_eq!(super::editing::last_page_offset(1000, 500), 500);
        assert_eq!(super::editing::last_page_offset(0, 500), 0);
    }

    #[test]
    fn undo_redo_steps_through_pending() {
        let mut rs = ResultSet::new(&Theme::dark(), None);
        let insert = |n: &str| PendingRowInsert {
            temp_id: n.into(),
            values: Default::default(),
        };
        rs.checkpoint();
        rs.pending.row_inserts.push(insert("a"));
        rs.checkpoint();
        rs.checkpoint(); // no-op change: not a step
        rs.pending.row_inserts.push(insert("b"));
        rs.step_history(false);
        assert_eq!(rs.pending.row_inserts.len(), 1);
        rs.step_history(false);
        assert!(rs.pending.is_empty());
        rs.step_history(false); // nothing left
        assert!(rs.pending.is_empty());
        rs.step_history(true);
        rs.step_history(true);
        rs.step_history(false);
        rs.checkpoint(); // a new change drops the redo branch
        rs.step_history(true);
        assert_eq!(rs.pending.row_inserts.len(), 1);
    }

    #[test]
    fn result_set_column_stats() {
        let theme = Theme::dark();
        let mut rs = ResultSet::new(&theme, None);
        use quarry_core::session::QueryResult;
        use quarry_core::value::DbValue;
        use std::time::Duration;

        let qr = QueryResult {
            statement: "SELECT id, name FROM users".into(),
            columns: vec!["id".into(), "name".into()],
            rows: vec![
                vec![DbValue::Int(1), DbValue::Text("Alice".into())],
                vec![DbValue::Int(2), DbValue::Text("Bob".into())],
                vec![DbValue::Int(3), DbValue::Text("Alice".into())],
            ]
            .into(),
            execution_time: Duration::from_millis(5),
            rows_affected: 3,
        };
        rs.set_results(vec![qr]);

        assert_eq!(rs.total_row_count(), 3);
        let id_stats = rs.compute_column_stats(0).expect("column 0 stats");
        assert_eq!(id_stats.column_name, "id");
        assert_eq!(id_stats.non_null_count, 3);
        assert_eq!(id_stats.null_count, 0);
        assert_eq!(id_stats.sum, Some(6.0));
        assert_eq!(id_stats.avg, Some(2.0));
        assert_eq!(id_stats.min_str.as_deref(), Some("1"));
        assert_eq!(id_stats.max_str.as_deref(), Some("3"));

        let name_stats = rs.compute_column_stats(1).expect("column 1 stats");
        assert_eq!(name_stats.column_name, "name");
        assert_eq!(name_stats.distinct_count, 2);
        assert_eq!(name_stats.min_str, None);
        assert_eq!(name_stats.max_str, None);
    }

    #[test]
    fn result_set_selection_range_and_stats() {
        let theme = Theme::dark();
        let mut rs = ResultSet::new(&theme, None);
        use quarry_core::session::QueryResult;
        use quarry_core::value::DbValue;
        use std::time::Duration;

        let qr = QueryResult {
            statement: "SELECT id, score FROM users".into(),
            columns: vec!["id".into(), "score".into()],
            rows: vec![
                vec![DbValue::Int(1), DbValue::Int(10)],
                vec![DbValue::Int(2), DbValue::Int(20)],
                vec![DbValue::Int(3), DbValue::Int(30)],
                vec![DbValue::Int(4), DbValue::Int(40)],
            ]
            .into(),
            execution_time: Duration::from_millis(5),
            rows_affected: 4,
        };
        rs.set_results(vec![qr]);

        // 1. Mouse down on row 1, col 1
        rs.handle_cell_mouse_down(1, 1, false);
        assert!(rs.is_dragging);
        assert_eq!(rs.selected_cell, Some((1, 1)));
        assert_eq!(rs.selected_range, Some(CellRange::single(1, 1)));

        // 2. Drag to row 3, col 1 (drag in column)
        rs.handle_cell_mouse_drag(3, 1);
        assert_eq!(rs.selected_cell, Some((3, 1)));
        let range = rs.selected_range.unwrap();
        assert_eq!(range.min_row(), 1);
        assert_eq!(range.max_row(), 3);
        assert_eq!(range.min_col(), 1);
        assert_eq!(range.max_col(), 1);
        assert_eq!(range.row_count(), 3);

        // 3. End drag
        rs.end_drag();
        assert!(!rs.is_dragging);

        // 4. Compute selection stats
        let stats = rs.compute_selection_stats(&range).expect("selection stats");
        assert_eq!(stats.non_null_count, 3);
        assert!(stats.is_numeric);
        assert_eq!(stats.sum, Some(90.0)); // 20 + 30 + 40
        assert_eq!(stats.avg, Some(30.0));
        assert_eq!(stats.min_str.as_deref(), Some("20"));
        assert_eq!(stats.max_str.as_deref(), Some("40"));

        let chips = stats.chips();
        assert_eq!(chips[0].label, "Count");
        assert_eq!(chips[0].display_value, "3");
        assert_eq!(chips[1].label, "Sum");
        assert_eq!(chips[1].display_value, "90");

        // 5. Copy range TSV
        let tsv = rs.copy_range_tsv(&range);
        assert_eq!(tsv, "20\n30\n40");

        // 6. Shift select to extend selection
        rs.handle_cell_mouse_down(0, 0, true);
        assert!(!rs.is_dragging);
        let ext_range = rs.selected_range.unwrap();
        assert_eq!(ext_range.min_row(), 0);
        assert_eq!(ext_range.max_row(), 1); // anchor was (1, 1), focus is (0, 0)
        assert_eq!(ext_range.min_col(), 0);
        assert_eq!(ext_range.max_col(), 1);
        assert_eq!(ext_range.cell_count(), 4);

        let tsv_box = rs.copy_range_tsv(&ext_range);
        assert_eq!(tsv_box, "1\t10\n2\t20");
    }
}
