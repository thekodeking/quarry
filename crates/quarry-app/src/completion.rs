// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Schema-aware autocomplete for the SQL editor (Q2): keywords, tables, and table.column.

use gpui::component::input::{CompletionProvider, HoverProvider, Rope, RopeExt};
use gpui::{App, Task, Window};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    CompletionTriggerKind, Hover, HoverContents, InlineCompletionContext, InlineCompletionItem,
    InlineCompletionResponse, MarkupContent, MarkupKind, TextEdit,
};
use quarry_core::SchemaCatalog;
use quarry_sql::{CompletionKind, SchemaCompleter};
use std::cell::RefCell;
use std::rc::Rc;

/// Shared with every editor; replaced whenever the active connection's schema changes
pub type SharedCatalog = Rc<RefCell<SchemaCatalog>>;
pub type SharedDialect = Rc<std::cell::Cell<&'static dyn quarry_core::Dialect>>;

/// Extracts the SQL text, local offset, and replacement range around the cursor.
/// For small documents (<= 16KB), returns the full text. For large documents,
/// slices a localized window around the cursor to avoid full buffer clones.
fn slice_completion_context(text: &Rope, offset: usize) -> (String, usize, lsp_types::Range) {
    let total_bytes = text.len();
    let offset = offset.min(total_bytes);

    if total_bytes <= 16 * 1024 {
        let sql = text.to_string();
        let start = sql[..offset]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let range = lsp_types::Range {
            start: text.offset_to_position(start),
            end: text.offset_to_position(offset),
        };
        (sql, offset, range)
    } else {
        // Find statement boundaries around offset to avoid cloning the entire buffer
        let scan_back = text.floor_char_boundary(offset.saturating_sub(8192));
        let back_slice = text.slice(scan_back..offset).to_string();
        let stmt_start = match back_slice.rfind(';') {
            Some(semi) => text.floor_char_boundary(scan_back + semi + 1),
            None => scan_back,
        };

        let scan_fwd = text.ceil_char_boundary((offset + 8192).min(total_bytes));
        let fwd_slice = text.slice(offset..scan_fwd).to_string();
        let stmt_end = match fwd_slice.find(';') {
            Some(semi) => text.ceil_char_boundary((offset + semi + 1).min(total_bytes)),
            None => scan_fwd,
        };

        let sql = text.slice(stmt_start..stmt_end).to_string();
        let local_offset = offset.saturating_sub(stmt_start).min(sql.len());
        let local_start = sql[..local_offset]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let abs_start = stmt_start + local_start;
        let range = lsp_types::Range {
            start: text.offset_to_position(abs_start),
            end: text.offset_to_position(offset),
        };
        (sql, local_offset, range)
    }
}

pub type SharedUsage = Rc<RefCell<std::collections::HashMap<String, u32>>>;
pub type SharedHistory = Rc<RefCell<Vec<String>>>;

pub struct SchemaCompletions {
    pub catalog: SharedCatalog,
    pub usage: Option<SharedUsage>,
    pub history: Option<SharedHistory>,
}

impl SchemaCompletions {
    pub fn new(
        catalog: SharedCatalog,
        usage: Option<SharedUsage>,
        history: Option<SharedHistory>,
    ) -> Self {
        Self {
            catalog,
            usage,
            history,
        }
    }
}

impl CompletionProvider for SchemaCompletions {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        ctx: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let (sql, local_offset, range) = slice_completion_context(text, offset);
        // Backspace past the start of a word (or onto a space) closes the menu instead of listing everything
        let in_word = sql[..local_offset]
            .chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
        if !in_word && ctx.trigger_kind != CompletionTriggerKind::INVOKED {
            return Task::ready(Ok(CompletionResponse::Array(vec![])));
        }
        let catalog = self.catalog.borrow().clone();
        let usage = self.usage.as_ref().map(|u| u.borrow().clone());
        cx.background_executor().spawn(async move {
            let items =
                SchemaCompleter::complete_with_usage(&sql, local_offset, &catalog, usage.as_ref())
                    .into_iter()
                    .map(|c| CompletionItem {
                        kind: Some(match c.kind {
                            CompletionKind::Keyword => CompletionItemKind::KEYWORD,
                            CompletionKind::Table => CompletionItemKind::STRUCT,
                            CompletionKind::Column => CompletionItemKind::FIELD,
                            CompletionKind::Schema => CompletionItemKind::MODULE,
                        }),
                        detail: c.detail,
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                            range,
                            new_text: c.label.clone(),
                        })),
                        label: c.label,
                        ..Default::default()
                    })
                    .collect();
            Ok(CompletionResponse::Array(items))
        })
    }

    /// Typing a word character or `.` (for table.column) opens suggestions; so does deleting
    /// (backspace), which `completions` answers only when the cursor is still inside a word
    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text.is_empty()
            || new_text
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
    }

    fn inline_completion_debounce(&self) -> std::time::Duration {
        std::time::Duration::from_millis(50)
    }

    fn inline_completion(
        &self,
        text: &Rope,
        offset: usize,
        _: InlineCompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<InlineCompletionResponse>> {
        let (sql, local_offset, _) = slice_completion_context(text, offset);
        let catalog = self.catalog.borrow().clone();
        let mut usage_map = self
            .usage
            .as_ref()
            .map(|u| u.borrow().clone())
            .unwrap_or_default();
        // Also boost identifier usage counts from current buffer
        for word in sql.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if !word.is_empty() {
                *usage_map.entry(word.to_lowercase()).or_default() += 1;
            }
        }
        let history_items = self.history.as_ref().map(|h| h.borrow().clone());
        cx.background_executor().spawn(async move {
            if let Some(insert_text) = evaluate_inline_completion(
                &sql,
                local_offset,
                &catalog,
                Some(&usage_map),
                history_items.as_deref(),
            ) {
                if !insert_text.is_empty() {
                    return Ok(InlineCompletionResponse::Array(vec![
                        InlineCompletionItem {
                            insert_text,
                            filter_text: None,
                            range: None,
                            command: None,
                            insert_text_format: None,
                        },
                    ]));
                }
            }
            Ok(InlineCompletionResponse::Array(vec![]))
        })
    }
}

/// Matches the current statement buffer against past queries in history (zsh/fish style).
/// Returns the remaining suffix if there is a clear, confident prefix match.
pub fn match_history_prefix(sql: &str, local_offset: usize, history: &[String]) -> Option<String> {
    if local_offset == 0 || local_offset > sql.len() {
        return None;
    }
    let before = &sql[..local_offset];
    let trimmed = before.trim();
    if trimmed.len() < 5 {
        return None;
    }

    let norm_typed: String = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    let norm_typed_lower = norm_typed.to_lowercase();

    for query in history {
        let clean_q = query.trim();
        let norm_q: String = clean_q.split_whitespace().collect::<Vec<_>>().join(" ");
        let norm_q_lower = norm_q.to_lowercase();

        if norm_q_lower.starts_with(&norm_typed_lower) && norm_q.len() > norm_typed.len() {
            let mut suffix = norm_q[norm_typed.len()..].to_string();
            if before.ends_with(' ') && suffix.starts_with(' ') {
                suffix = suffix.trim_start().to_string();
            }
            if let Some(semi_pos) = suffix.find(';') {
                suffix.truncate(semi_pos + 1);
            }
            if suffix.len() > 120 {
                if let Some(space_pos) = suffix[..120].rfind(' ') {
                    suffix.truncate(space_pos);
                }
            }
            if !suffix.trim().is_empty() {
                return Some(suffix);
            }
        }
    }
    None
}

/// Evaluates inline completion suggestions (ghost text) with confidence gating,
/// clause continuations, keyword skipping, query history matching, and catalog casing preservation.
pub fn evaluate_inline_completion(
    sql: &str,
    local_offset: usize,
    catalog: &SchemaCatalog,
    usage: Option<&std::collections::HashMap<String, u32>>,
    history: Option<&[String]>,
) -> Option<String> {
    if local_offset == 0 || local_offset > sql.len() {
        return None;
    }

    let before_cursor = &sql[..local_offset];

    // 1. Clause keyword check after space (e.g. `JOIN ` or `ON `)
    let trimmed = before_cursor.trim_end();
    let ends_with_space = before_cursor.len() > trimmed.len();
    let last_word = trimmed
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_lowercase();

    if ends_with_space && (last_word == "join" || last_word == "on") {
        let completions = SchemaCompleter::complete_with_usage(sql, local_offset, catalog, usage);
        if last_word == "join" {
            let fk_candidates: Vec<_> = completions
                .into_iter()
                .filter(|c| c.label.contains(" ON "))
                .collect();
            if !fk_candidates.is_empty() {
                let confident = if fk_candidates.len() == 1 {
                    true
                } else {
                    let count_0 = usage
                        .and_then(|u| u.get(&fk_candidates[0].label.to_lowercase()))
                        .copied()
                        .unwrap_or(0);
                    let count_1 = usage
                        .and_then(|u| u.get(&fk_candidates[1].label.to_lowercase()))
                        .copied()
                        .unwrap_or(0);
                    count_0 > count_1
                };
                if confident {
                    return Some(fk_candidates[0].label.clone());
                }
            }
        } else if last_word == "on" {
            let on_candidates: Vec<_> = completions
                .into_iter()
                .filter(|c| c.detail.as_deref().unwrap_or("").contains("foreign key"))
                .collect();
            if !on_candidates.is_empty() {
                let confident = if on_candidates.len() == 1 {
                    true
                } else {
                    let count_0 = usage
                        .and_then(|u| u.get(&on_candidates[0].label.to_lowercase()))
                        .copied()
                        .unwrap_or(0);
                    let count_1 = usage
                        .and_then(|u| u.get(&on_candidates[1].label.to_lowercase()))
                        .copied()
                        .unwrap_or(0);
                    count_0 > count_1
                };
                if confident {
                    return Some(on_candidates[0].label.clone());
                }
            }
        }
    }

    // 2. Query History Matcher (zsh/fish-style autosuggestion for recurring queries)
    if let Some(history) = history {
        if let Some(ghost) = match_history_prefix(sql, local_offset, history) {
            return Some(ghost);
        }
    }

    // 3. Continuation check (e.g. `LIMIT 100`, `WHERE `, `GROUP BY`, `ORDER BY`, `= `, `IS `)
    if let Some(cont) = SchemaCompleter::continuation(sql, local_offset, catalog) {
        if !cont.is_empty() {
            return Some(cont);
        }
    }

    // 4. Normal token completion while typing
    let last_char = before_cursor.chars().last();
    let is_typing_token = last_char.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
    if !is_typing_token {
        return None;
    }

    let prefix_start = before_cursor
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    let prefix = &before_cursor[prefix_start..];

    let completions = SchemaCompleter::complete_with_usage(sql, local_offset, catalog, usage);
    if completions.is_empty() {
        return None;
    }

    let best = &completions[0];

    // Rule 3: Skip bare SQL keywords (e.g. typing `se` ghosts `lect`)
    if best.kind == CompletionKind::Keyword && !best.label.contains(' ') {
        return None;
    }

    // Rule 2: Confidence gate
    if completions.len() > 1 {
        let runner_up = &completions[1];
        let prefix_lower = prefix.to_lowercase();
        let score_best = quarry_sql::match_score(&prefix_lower, &best.label).unwrap_or(u8::MAX);
        let score_runner =
            quarry_sql::match_score(&prefix_lower, &runner_up.label).unwrap_or(u8::MAX);
        let count_best = usage
            .and_then(|u| u.get(&best.label.to_lowercase()))
            .copied()
            .unwrap_or(0);
        let count_runner = usage
            .and_then(|u| u.get(&runner_up.label.to_lowercase()))
            .copied()
            .unwrap_or(0);

        let strictly_better =
            score_best < score_runner || (score_best == score_runner && count_best > count_runner);
        if !strictly_better {
            return None;
        }
    }

    if prefix.is_empty() {
        if before_cursor.ends_with('.') {
            Some(best.label.clone())
        } else {
            None
        }
    } else if best
        .label
        .to_lowercase()
        .starts_with(&prefix.to_lowercase())
        && best.label.len() > prefix.len()
    {
        let raw_suffix = &best.label[prefix.len()..];
        // Rule 6: Case from the catalog. Only keywords follow typing case.
        let suffix = if best.kind == CompletionKind::Keyword {
            let is_all_upper = prefix
                .chars()
                .all(|c| !c.is_alphabetic() || c.is_uppercase());
            let is_all_lower = prefix
                .chars()
                .all(|c| !c.is_alphabetic() || c.is_lowercase());
            if is_all_upper {
                raw_suffix.to_uppercase()
            } else if is_all_lower {
                raw_suffix.to_lowercase()
            } else {
                raw_suffix.to_string()
            }
        } else {
            // Identifiers keep catalog casing (e.g. MixedCase, Oracle UPPERCASE)
            raw_suffix.to_string()
        };
        Some(suffix)
    } else {
        None
    }
}

/// Squiggles under tables and `alias.column`s the connected database doesn't have.
/// ponytail: re-lints on edit only; switching connection leaves old squiggles until the next keystroke.
pub fn lint_editor(
    editor: &gpui::Entity<gpui::component::input::EditorState>,
    catalog: &SharedCatalog,
    cx: &mut App,
) {
    use gpui::base::input::{Diagnostic, DiagnosticSeverity};
    editor.update(cx, |e, cx| {
        if e.value().len() > 200_000 {
            if let Some(set) = e.diagnostics_mut() {
                set.reset(&Rope::from(""));
                cx.notify();
            }
            return;
        }
        let sql = e.value().to_string();
        let rope = Rope::from(sql.as_str());
        let found = quarry_sql::lint(&sql, &catalog.borrow());
        let Some(set) = e.diagnostics_mut() else {
            return;
        };
        set.reset(&rope);
        for (range, message) in found {
            set.push(
                Diagnostic::new(
                    rope.offset_to_position(range.start)..rope.offset_to_position(range.end),
                    message,
                )
                .with_severity(DiagnosticSeverity::Warning),
            );
        }
        cx.notify();
    });
}

/// Error squiggle for a failed run, kept until the next edit re-lints the editor.
pub fn mark_run_error(
    editor: &gpui::Entity<gpui::component::input::EditorState>,
    stmt: &str,
    at: Option<usize>,
    message: &str,
    cx: &mut App,
) {
    use gpui::base::input::{Diagnostic, DiagnosticSeverity};
    editor.update(cx, |e, cx| {
        let sql = e.value().to_string();
        let Some(base) = sql.find(stmt.trim()) else {
            return;
        };
        let stmt = stmt.trim();
        let range = error_range(stmt, at);
        let rope = Rope::from(sql.as_str());
        let Some(set) = e.diagnostics_mut() else {
            return;
        };
        set.push(
            Diagnostic::new(
                rope.offset_to_position(base + range.start)
                    ..rope.offset_to_position(base + range.end),
                message.to_string(),
            )
            .with_severity(DiagnosticSeverity::Error),
        );
        cx.notify();
    });
}

/// Byte range to underline: the word at character `at`, else the statement's first line
fn error_range(stmt: &str, at: Option<usize>) -> std::ops::Range<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '"' || c == '.';
    let start = at
        .and_then(|n| stmt.char_indices().nth(n.saturating_sub(1)))
        .map(|(i, _)| i);
    match start {
        Some(s) => {
            let end = stmt[s..]
                .find(|c| !is_word(c))
                .map_or(stmt.len(), |e| s + e);
            s..end.max(s + stmt[s..].chars().next().map_or(0, char::len_utf8))
        }
        None => 0..stmt.find('\n').unwrap_or(stmt.len()),
    }
}

/// ⌥↵ / ⌘. on a statement: format, expand `*`, add LIMIT, EXPLAIN, wrap in a transaction
pub struct SqlActions(pub SharedCatalog, pub SharedDialect);

/// (title, rewritten statement)
type Rewrite = (String, String);

impl SqlActions {
    /// The statement at `offset` and every (title, rewrite) that applies to it
    fn for_statement(
        &self,
        sql: &str,
        offset: usize,
    ) -> Option<(std::ops::Range<usize>, Vec<Rewrite>)> {
        let span = quarry_sql::find_statement_at_offset(sql, offset.min(sql.len()))?;
        let stmt = &sql[span.start..span.end];
        let mut actions = vec![(
            "Format statement".to_string(),
            crate::files::format_sql(stmt),
        )];
        actions.extend(
            quarry_sql::rewrites(stmt, &self.0.borrow(), self.1.get())
                .into_iter()
                .map(|(t, s)| (t.to_string(), s)),
        );
        actions.retain(|(_, s)| s != stmt);
        Some((span.start..span.end, actions))
    }
}

impl gpui::component::input::CodeActionProvider for SqlActions {
    fn id(&self) -> gpui::SharedString {
        "quarry-sql".into()
    }

    // Called outside the editor's update, so reading it is safe here
    fn code_actions(
        &self,
        state: gpui::Entity<gpui::component::input::EditorState>,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<lsp_types::CodeAction>>> {
        let sql = state.read(cx).value().to_string();
        let actions = self
            .for_statement(&sql, range.start)
            .map(|(_, a)| a)
            .unwrap_or_default();
        Task::ready(Ok(actions
            .into_iter()
            .map(|(title, _)| lsp_types::CodeAction {
                title,
                ..Default::default()
            })
            .collect()))
    }

    // Called inside the editor's update: edit it on the next tick, not now
    fn perform_code_action(
        &self,
        state: gpui::Entity<gpui::component::input::EditorState>,
        action: lsp_types::CodeAction,
        _: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        let (catalog, dialect) = (self.0.clone(), self.1.clone());
        window.defer(cx, move |window, cx| {
            state.update(cx, |e, cx| {
                let sql = e.value().to_string();
                let Some((range, actions)) =
                    SqlActions(catalog, dialect).for_statement(&sql, e.cursor())
                else {
                    return;
                };
                let Some((_, text)) = actions.into_iter().find(|(t, _)| *t == action.title) else {
                    return;
                };
                e.set_selected_range(range, cx);
                e.replace(text, window, cx); // one undo step
            });
        });
        Task::ready(Ok(()))
    }
}

/// ⌘-click a table name to open it
pub struct TableDefinitions(pub SharedCatalog);

impl gpui::component::input::DefinitionProvider for TableDefinitions {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        _: &mut App,
    ) -> Task<anyhow::Result<Vec<lsp_types::LocationLink>>> {
        let sql = text.to_string();
        let catalog = self.0.borrow();
        let offset = offset.min(sql.len());
        let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == '"';
        let start = sql[..offset].rfind(|c| !is_ident(c)).map_or(0, |i| i + 1);
        let end = sql[offset..]
            .find(|c| !is_ident(c))
            .map_or(sql.len(), |i| offset + i);
        let word = sql[start..end].replace('"', "");
        let stmt = quarry_sql::find_statement_at_offset(&sql, offset)
            .map_or(sql.as_str(), |s| &sql[s.start..s.end]);
        let refs = quarry_sql::table_refs(stmt, &catalog);
        // A table name, `schema.table`, or an alias standing for one
        let table = match word.split_once('.') {
            Some((s, t)) => catalog.find_table(Some(s), t),
            None => quarry_sql::resolve_qualifier(&word, &refs, &catalog),
        };
        let link = table.and_then(|t| {
            let uri: lsp_types::Uri = format!("quarry://table/{}/{}", t.schema_name, t.table_name)
                .parse()
                .ok()?;
            let range = lsp_types::Range {
                start: text.offset_to_position(start),
                end: text.offset_to_position(end),
            };
            Some(lsp_types::LocationLink {
                origin_selection_range: Some(range),
                target_uri: uri,
                target_range: Default::default(),
                target_selection_range: Default::default(),
            })
        });
        Task::ready(Ok(link.into_iter().collect()))
    }
}

/// `quarry://table/<schema>/<table>` → (schema, table)
pub fn table_link(uri: &lsp_types::Uri) -> Option<(String, String)> {
    let s = uri.as_str().strip_prefix("quarry://table/")?;
    let (schema, table) = s.split_once('/')?;
    Some((schema.to_string(), table.to_string()))
}

/// Hovering a table shows its columns; hovering a column shows its type and keys
pub struct SchemaHover(pub SharedCatalog);

impl HoverProvider for SchemaHover {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        _: &mut App,
    ) -> Task<anyhow::Result<Option<Hover>>> {
        let hover =
            hover_markdown(&text.to_string(), offset, &self.0.borrow()).map(|value| Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: None,
            });
        Task::ready(Ok(hover))
    }
}

/// The dotted identifier under `offset` (`schema.table`, `t.col`, `col`), looked up in the catalog
fn hover_markdown(sql: &str, offset: usize, catalog: &SchemaCatalog) -> Option<String> {
    let offset = offset.min(sql.len());
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == '"';
    let start = sql[..offset].rfind(|c| !is_ident(c)).map_or(0, |i| i + 1);
    let end = sql[offset..]
        .find(|c| !is_ident(c))
        .map_or(sql.len(), |i| offset + i);
    let word = sql[start..end].replace('"', "");
    let parts: Vec<&str> = word.split('.').filter(|p| !p.is_empty()).collect();
    let table_md = |t: &quarry_core::TableSchema| {
        let cols: Vec<String> = t
            .columns
            .iter()
            .map(|c| {
                format!(
                    "- `{}` {}{}",
                    c.name,
                    c.raw_type_name.to_lowercase(),
                    keys(t, c)
                )
            })
            .collect();
        format!(
            "**{}.{}**{}\n\n{}",
            t.schema_name,
            t.table_name,
            if t.is_view { " (view)" } else { "" },
            cols.join("\n")
        )
    };
    let column_md = |t: &quarry_core::TableSchema, c: &quarry_core::ColumnSchema| {
        format!(
            "`{}.{}` **{}**{}{}",
            t.table_name,
            c.name,
            c.raw_type_name.to_lowercase(),
            if c.is_nullable { "" } else { " not null" },
            keys(t, c)
        )
    };
    let stmt =
        quarry_sql::find_statement_at_offset(sql, offset).map_or(sql, |s| &sql[s.start..s.end]);
    let refs = quarry_sql::table_refs(stmt, catalog);
    // A bare column: only answer when exactly one table has it
    let bare_column = |name: &str| {
        let mut hits = catalog.tables.iter().flat_map(|t| {
            t.columns
                .iter()
                .filter(|c| c.name.eq_ignore_ascii_case(name))
                .map(move |c| (t, c))
        });
        match (hits.next(), hits.next()) {
            (Some((t, c)), None) => Some(column_md(t, c)),
            _ => None,
        }
    };
    match parts.as_slice() {
        [name] => catalog
            .find_table(None, name)
            .map(table_md)
            .or_else(|| bare_column(name)),
        [a, b] => catalog
            .find_table(Some(a), b)
            .map(table_md)
            .or_else(|| {
                quarry_sql::resolve_qualifier(a, &refs, catalog).and_then(|t| {
                    t.columns
                        .iter()
                        .find(|c| c.name.eq_ignore_ascii_case(b))
                        .map(|c| column_md(t, c))
                })
            })
            .or_else(|| bare_column(b)),
        [s, t, c] => catalog.find_table(Some(s), t).and_then(|t| {
            t.columns
                .iter()
                .find(|col| col.name.eq_ignore_ascii_case(c))
                .map(|col| column_md(t, col))
        }),
        _ => None,
    }
}

fn keys(t: &quarry_core::TableSchema, c: &quarry_core::ColumnSchema) -> String {
    let mut out = String::new();
    if c.is_primary_key {
        out.push_str(" · PK");
    }
    if let Some(fk) = t.foreign_keys.iter().find(|f| f.from_column == c.name) {
        out.push_str(&format!(" · FK → {}.{}", fk.to_table, fk.to_column));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{evaluate_inline_completion, hover_markdown};
    use quarry_core::schema::{ColumnSchema, ColumnType, SchemaCatalog, TableSchema};

    fn col(name: &str, pk: bool) -> ColumnSchema {
        ColumnSchema {
            name: name.into(),
            data_type: ColumnType::Integer,
            raw_type_name: "INT".into(),
            is_nullable: !pk,
            is_primary_key: pk,
            default_value: None,
            ordinal_position: 1,
        }
    }

    #[test]
    fn underlines_the_reported_word() {
        let stmt = "select * form users";
        assert_eq!(&stmt[super::error_range(stmt, Some(10))], "form");
        let two_lines = "select 1\nfrom x";
        assert_eq!(&two_lines[super::error_range(two_lines, None)], "select 1");
        // A position on punctuation still underlines one character
        assert_eq!(&stmt[super::error_range(stmt, Some(8))], "*");
    }

    #[test]
    fn hovers_tables_and_columns() {
        let t = |name: &str, cols| TableSchema {
            schema_name: "public".into(),
            table_name: name.into(),
            is_view: false,
            columns: cols,
            primary_keys: vec![],
            foreign_keys: vec![],
        };
        let cat = SchemaCatalog {
            schemas: vec!["public".into()],
            tables: vec![
                t("users", vec![col("id", true), col("email", false)]),
                t("orders", vec![col("id", true)]),
            ],
            default_schema: None,
            ..Default::default()
        };
        let sql = "select u.email, id from users u join public.orders o";
        let at = |w: &str| sql.find(w).unwrap() + 1;
        assert!(hover_markdown(sql, at("users"), &cat)
            .unwrap()
            .starts_with("**public.users**"));
        assert!(hover_markdown(sql, at("public.orders"), &cat)
            .unwrap()
            .starts_with("**public.orders**"));
        assert!(hover_markdown(sql, at("email"), &cat)
            .unwrap()
            .contains("`users.email` **int**"));
        // `id` is in two tables, so a bare hover can't say which
        assert_eq!(hover_markdown(sql, at("id from"), &cat), None);
        assert_eq!(hover_markdown(sql, at("select"), &cat), None);
    }

    #[test]
    fn inline_completion_rules() {
        let col = |name: &str| ColumnSchema {
            name: name.into(),
            data_type: ColumnType::Integer,
            raw_type_name: "int".into(),
            is_nullable: false,
            is_primary_key: false,
            default_value: None,
            ordinal_position: 1,
        };
        let t = |name: &str, cols, fks| TableSchema {
            schema_name: "public".into(),
            table_name: name.into(),
            is_view: false,
            columns: cols,
            primary_keys: vec!["id".into()],
            foreign_keys: fks,
        };
        let fk = quarry_core::schema::ForeignKeyRef {
            from_column: "customer_id".into(),
            to_schema: "public".into(),
            to_table: "customers".into(),
            to_column: "id".into(),
        };
        let cat = SchemaCatalog {
            schemas: vec!["public".into()],
            tables: vec![
                t("customers", vec![col("id"), col("email")], vec![]),
                t(
                    "orders",
                    vec![col("id"), col("customer_id"), col("total")],
                    vec![fk],
                ),
                t("CUSTOMERS_UPPER", vec![col("id")], vec![]),
            ],
            default_schema: None,
            ..Default::default()
        };

        // 1. JOIN with one FK ghosts the clause
        let sql_join = "SELECT * FROM customers c JOIN ";
        assert_eq!(
            evaluate_inline_completion(sql_join, sql_join.len(), &cat, None, None),
            Some("orders ON orders.customer_id = c.id".into())
        );

        // 2. Bare keyword prefix ghosts nothing
        let sql_kw = "se";
        assert_eq!(
            evaluate_inline_completion(sql_kw, sql_kw.len(), &cat, None, None),
            None
        );

        // 3. Two equal candidates ghost nothing (confidence gate)
        let sql_ambiguous = "SELECT * FROM orders WHERE c";
        // Candidates `customer_id` vs keywords like `CASE`, `COUNT` etc.
        // If ambiguous (multiple matching), confidence gate blocks it
        let res = evaluate_inline_completion(sql_ambiguous, sql_ambiguous.len(), &cat, None, None);
        assert_eq!(res, None);

        // 4. History-heavy column wins a tie
        let mut usage = std::collections::HashMap::new();
        usage.insert("customer_id".into(), 100);
        let res_boosted = evaluate_inline_completion(
            sql_ambiguous,
            sql_ambiguous.len(),
            &cat,
            Some(&usage),
            None,
        );
        assert_eq!(res_boosted, Some("ustomer_id".into()));

        // 5. Oracle UPPERCASE name keeps its case
        let sql_oracle = "SELECT * FROM cu";
        // Boost CUSTOMERS_UPPER so it's confident
        let mut oracle_usage = std::collections::HashMap::new();
        oracle_usage.insert("customers_upper".into(), 100);
        let oracle_ghost = evaluate_inline_completion(
            sql_oracle,
            sql_oracle.len(),
            &cat,
            Some(&oracle_usage),
            None,
        );
        assert_eq!(oracle_ghost, Some("STOMERS_UPPER".into()));

        // 6. Continuation LIMIT 100 on SELECT
        let sql_select = "SELECT * FROM customers ";
        assert_eq!(
            evaluate_inline_completion(sql_select, sql_select.len(), &cat, None, None),
            Some("LIMIT 100".into())
        );

        // 7. Query history matching (zsh/fish autosuggestion)
        let history = vec!["SELECT id, email FROM customers WHERE id = 42;".to_string()];
        let sql_hist = "SELECT id, email FROM customers ";
        assert_eq!(
            evaluate_inline_completion(sql_hist, sql_hist.len(), &cat, None, Some(&history)),
            Some("WHERE id = 42;".into())
        );
    }
}
