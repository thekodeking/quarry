// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! SQL files and session restore (Q8, Q9): format, open/save .sql, and reopen unsaved query tabs.

use crate::state::Tab;
use crate::QuarryWindow;
use gpui::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
struct SavedTab {
    title: String,
    sql: String,
    file: Option<PathBuf>,
    #[serde(default)]
    connection_id: Option<String>,
    /// A table tab: reopened once its connection is connected
    #[serde(default)]
    table: Option<SavedTable>,
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    row_limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SavedTable {
    pub schema: String,
    pub table: String,
    pub view: crate::table_detail::TableView,
}

/// Where Save As starts, and what the sidebar's Queries section lists
pub fn queries_dir() -> Option<PathBuf> {
    Some(crate::connections::config_dir()?.join("queries"))
}

/// `.sql` files in the queries folder, by name. Re-read only when the folder's mtime changes
/// (adding, removing or renaming a file bumps it), since the sidebar asks on every render.
pub fn saved_queries() -> Vec<PathBuf> {
    thread_local! {
        static CACHE: std::cell::RefCell<Option<(std::time::SystemTime, Vec<PathBuf>)>> = const { std::cell::RefCell::new(None) };
    }
    let Some(dir) = queries_dir() else {
        return vec![];
    };
    let Ok(mtime) = std::fs::metadata(&dir).and_then(|m| m.modified()) else {
        return vec![];
    };
    CACHE.with_borrow_mut(|cache| {
        if let Some((_, files)) = cache.as_ref().filter(|(at, _)| *at == mtime) {
            return files.clone();
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "sql"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        *cache = Some((mtime, files.clone()));
        files
    })
}

fn recent_path() -> Option<PathBuf> {
    Some(crate::connections::config_dir()?.join("recent_files.json"))
}

/// .sql files opened or saved outside the queries folder, newest first (shown under "Recent")
pub fn recent_files() -> Vec<PathBuf> {
    let list: Vec<PathBuf> = recent_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    list.into_iter().filter(|p| p.exists()).collect()
}

pub fn remember_file(path: &std::path::Path) {
    let in_queries = queries_dir().is_some_and(|d| path.parent() == Some(d.as_path()));
    if in_queries || path.extension().is_none_or(|e| e != "sql") {
        return;
    }
    let mut list = recent_files();
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(10);
    if let (Some(p), Ok(json)) = (recent_path(), serde_json::to_string_pretty(&list)) {
        let _ = std::fs::write(p, json);
    }
}

pub(crate) fn session_path() -> Option<PathBuf> {
    Some(crate::connections::config_dir()?.join("session.json"))
}

/// Pretty-prints SQL; keeps the user's keyword case
pub fn format_sql(sql: &str) -> String {
    let opts = sqlformat::FormatOptions {
        lines_between_queries: 2,
        ..Default::default()
    };
    sqlformat::format(sql, &sqlformat::QueryParams::None, &opts)
}

impl QuarryWindow {
    /// Formats the selection, or the whole query
    pub fn format_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Tab::Query { editor, .. }) = self.state.active_tab() else {
            return;
        };
        let editor = editor.clone();
        let (text, sel) = {
            let e = editor.read(cx);
            (e.value().to_string(), e.selected_range())
        };
        let formatted = if sel.is_empty() {
            format_sql(&text)
        } else {
            format!(
                "{}{}{}",
                &text[..sel.start],
                format_sql(&text[sel.clone()]),
                &text[sel.end..]
            )
        };
        editor.update(cx, |e, cx| e.set_value(formatted, window, cx));
    }

    /// ⌘S on a query tab: write to its file, or ask where (Save As) the first time
    pub fn save_query(&mut self, save_as: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.state.active_tab_index;
        let Some(Tab::Query {
            editor,
            file,
            title,
            ..
        }) = self.state.tabs.get(ix)
        else {
            return;
        };
        let text = editor.read(cx).value().to_string();
        let (existing, name) = (
            file.clone(),
            format!("{}.sql", title.trim_end_matches(".sql").replace(' ', "_")),
        );
        match existing.filter(|_| !save_as) {
            Some(path) => self.write_query(ix, path, text, cx),
            None => {
                let dir = queries_dir()
                    .filter(|d| std::fs::create_dir_all(d).is_ok())
                    .or_else(std::env::home_dir)
                    .unwrap_or_default();
                let rx = cx.prompt_for_new_path(&dir, Some(&name));
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(Ok(Some(path))) = rx.await else { return };
                    this.update(cx, |this, cx| this.write_query(ix, path, text, cx))
                        .ok();
                })
                .detach();
            }
        }
    }

    fn write_query(&mut self, ix: usize, path: PathBuf, text: String, cx: &mut Context<Self>) {
        if let Err(e) = std::fs::write(&path, &text) {
            return self.toast(format!("Couldn't save: {e}"), true, cx);
        }
        remember_file(&path);
        if let Some(Tab::Query {
            file,
            title,
            saved_text,
            ..
        }) = self.state.tabs.get_mut(ix)
        {
            *title = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            *file = Some(path.clone());
            *saved_text = text;
        }
        self.save_session(cx);
        // Saving a config file applies it right away
        if crate::settings::settings_path().is_some_and(|p| p == path) {
            self.reload_settings(cx);
            self.toast("Settings applied", false, cx);
        } else if crate::settings::keymap_path().is_some_and(|p| p == path) {
            match crate::settings::apply_keymap(cx).first() {
                Some(e) => self.toast(e.clone(), true, cx),
                None => self.toast("Keymap applied", false, cx),
            }
        }
        cx.notify();
    }

    /// Opens a saved query, or switches to its tab if it's already open
    pub fn open_query_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self
            .state
            .tabs
            .iter()
            .position(|t| matches!(t, Tab::Query { file: Some(f), .. } if *f == path))
        {
            self.state.active_tab_index = ix;
        } else {
            match std::fs::read_to_string(&path) {
                Ok(sql) => self.open_query_tab_with(&sql, Some(path), window, cx),
                Err(e) => self.toast(format!("Couldn't open {}: {}", path.display(), e), true, cx),
            }
        }
        cx.notify();
    }

    /// Rename… on a saved query: the sidebar row becomes a name box; Enter renames, Esc or clicking away cancels
    pub fn rename_query_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let input =
            cx.new(|cx| gpui::component::input::InputState::new(window, cx).default_value(stem));
        input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, ev: &gpui::component::input::InputEvent, window, cx| {
                use gpui::component::input::InputEvent;
                match ev {
                    InputEvent::PressEnter { .. } => {
                        let name = input.read(cx).value().trim().to_string();
                        this.finish_rename(Some(name), window, cx)
                    }
                    InputEvent::Blur => this.finish_rename(None, window, cx),
                    _ => {}
                }
            },
        );
        self.tab_subscriptions.insert("rename-query".into(), sub);
        self.state.renaming_query = Some((path, input));
        cx.notify();
    }

    /// `name`: the new name without `.sql`; `None` cancels
    pub fn finish_rename(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((from, _)) = self.state.renaming_query.take() else {
            return;
        };
        self.tab_subscriptions.remove("rename-query");
        self.refocus_root(window, cx);
        cx.notify();
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            return;
        };
        if name.contains('/') {
            return self.toast("A name can't contain /", true, cx);
        }
        let to = from.with_file_name(format!("{}.sql", name.trim_end_matches(".sql")));
        if to == from {
            return;
        }
        if to.exists() {
            return self.toast(format!("{} already exists", to.display()), true, cx);
        }
        if let Err(e) = std::fs::rename(&from, &to) {
            return self.toast(format!("Couldn't rename: {e}"), true, cx);
        }
        remember_file(&to);
        self.retarget_query_tabs(&from, Some(to), cx);
    }

    /// Duplicate: `name copy.sql`, `name copy 2.sql`, … next to the original
    pub fn duplicate_query_file(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let dir = path.parent().map(PathBuf::from).unwrap_or_default();
        let to = (1..)
            .map(|n| {
                dir.join(if n == 1 {
                    format!("{stem} copy.sql")
                } else {
                    format!("{stem} copy {n}.sql")
                })
            })
            .find(|p| !p.exists())
            .expect("unbounded");
        match std::fs::copy(path, &to) {
            Ok(_) => remember_file(&to),
            Err(e) => self.toast(format!("Couldn't duplicate: {e}"), true, cx),
        }
        cx.notify();
    }

    /// Move to Trash / Recycle Bin, so it can be put back; an open tab keeps its text as an unsaved query
    pub fn trash_query_file(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        #[allow(unused_mut)]
        let mut trash = trash::TrashContext::default();
        // The default asks Finder over AppleScript, which blocks on an Automation prompt
        #[cfg(target_os = "macos")]
        {
            use trash::macos::{DeleteMethod, TrashContextExtMacos};
            trash.set_delete_method(DeleteMethod::NsFileManager);
        }
        match trash.delete(path) {
            Ok(()) => self.retarget_query_tabs(path, None, cx),
            Err(e) => self.toast(format!("Couldn't move to Trash: {e}"), true, cx),
        }
    }

    /// Points tabs open on `from` at `to` (rename), or detaches them (deleted)
    fn retarget_query_tabs(
        &mut self,
        from: &std::path::Path,
        to: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        for tab in &mut self.state.tabs {
            if let Tab::Query {
                file,
                title,
                saved_text,
                ..
            } = tab
            {
                if file.as_deref() != Some(from) {
                    continue;
                }
                match &to {
                    Some(to) => {
                        *title = to
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default()
                    }
                    None => saved_text.clear(), // shows as unsaved
                }
                *file = to.clone();
            }
        }
        self.save_session(cx);
        cx.notify();
    }

    /// ⌘O: open a .sql file in a new query tab
    pub fn open_sql_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                for path in paths {
                    // A SQLite database opens as a connection, not as text
                    if is_sqlite_db(&path) {
                        this.connect_address(&path.to_string_lossy(), window, cx);
                        continue;
                    }
                    match std::fs::read_to_string(&path) {
                        Ok(sql) => {
                            remember_file(&path);
                            this.open_query_tab_with(&sql, Some(path), window, cx)
                        }
                        Err(e) => {
                            this.toast(format!("Couldn't open {}: {}", path.display(), e), true, cx)
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Streams a `.sql` file from disk and executes its statements without loading it into an editor
    pub fn run_sql_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.state.session.clone() else {
            return self.toast("Connect to a database first", true, cx);
        };
        let rules = self.state.dialect().split_rules();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Run SQL File".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let name = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            let _ = this.update_in(cx, |this, window, cx| {
                this.toast(format!("Running {name}…"), false, cx);
                let fut = async move {
                    let started = std::time::Instant::now();
                    let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                    let reader = std::io::BufReader::new(file);
                    let mut count = 0usize;
                    for stmt in quarry_sql::StatementStream::new(reader, rules) {
                        let sql = stmt.map_err(|e| e.to_string())?;
                        session
                            .execute(&sql)
                            .await
                            .map_err(|e| format!("Statement {}: {e}", count + 1))?;
                        count += 1;
                    }
                    Ok::<(usize, u128), String>((count, started.elapsed().as_millis()))
                };
                crate::runtime::run_db(window, cx, fut, move |this, res, window, cx| match res {
                    Ok((count, ms)) => {
                        let msg = format!("Executed {count} statements from {name} in {ms}ms");
                        this.state.messages.push(msg.clone());
                        this.toast(msg, false, cx);
                        this.refresh_catalog(window, cx);
                    }
                    Err(e) => {
                        let msg = format!("{name} failed: {e}");
                        this.state.messages.push(msg.clone());
                        this.toast(msg, true, cx);
                    }
                });
            });
        })
        .detach();
    }

    /// Writes open query tabs to disk shortly after typing stops, so nothing is lost
    pub fn schedule_session_save(&mut self, cx: &mut Context<Self>) {
        self.session_save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            this.update(cx, |this, cx| this.save_session(cx)).ok();
        }));
    }

    pub fn save_session(&mut self, cx: &mut Context<Self>) {
        let tabs: Vec<SavedTab> = self
            .state
            .tabs
            .iter()
            .map(|t| match t {
                Tab::Query {
                    title,
                    editor,
                    file,
                    connection_id,
                    schema,
                    row_limit,
                    ..
                } => SavedTab {
                    title: title.clone(),
                    sql: editor.read(cx).value().to_string(),
                    file: file.clone(),
                    connection_id: connection_id.clone(),
                    table: None,
                    schema: schema.clone(),
                    row_limit: *row_limit,
                },
                Tab::Table {
                    schema_name,
                    table_name,
                    connection,
                    view,
                    ..
                } => SavedTab {
                    title: table_name.clone(),
                    sql: String::new(),
                    file: None,
                    connection_id: connection.as_ref().map(|c| c.id.clone()),
                    table: Some(SavedTable {
                        schema: schema_name.clone(),
                        table: table_name.clone(),
                        view: *view,
                    }),
                    schema: None,
                    row_limit: None,
                },
            })
            // Still waiting for their connection: keep them for next time
            .chain(self.pending_table_tabs.iter().map(|(conn, t)| SavedTab {
                title: t.table.clone(),
                sql: String::new(),
                file: None,
                connection_id: Some(conn.clone()),
                table: Some(t.clone()),
                schema: None,
                row_limit: None,
            }))
            .collect();
        let Some(path) = session_path() else { return };
        let _ = std::fs::create_dir_all(path.parent().expect("has parent"));
        if let Ok(json) = serde_json::to_string_pretty(&tabs) {
            let _ = std::fs::write(path, json);
        }
    }

    /// Table tabs from last session wait for their connection; once it's the active one, open them
    pub fn reopen_pending_table_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.state.active_connection.as_ref().map(|c| c.id.clone()) else {
            return;
        };
        if self.state.catalog.tables.is_empty()
            || !self.pending_table_tabs.iter().any(|(c, _)| *c == active)
        {
            return;
        }
        let keep = self.state.active_tab_index;
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_table_tabs)
            .into_iter()
            .partition(|(c, _)| *c == active);
        self.pending_table_tabs = later;
        for (_, t) in now {
            self.open_table_tab(&t.table, &t.schema, window, cx);
            self.refresh_active_tab(window, cx);
            if t.view != crate::table_detail::TableView::Data {
                self.set_table_view(t.view, window, cx);
            }
        }
        self.state.active_tab_index = keep.min(self.state.tabs.len().saturating_sub(1));
    }

    /// Reopens last session's query tabs (unsaved text included)
    pub fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(saved) = session_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Vec<SavedTab>>(&s).ok())
        else {
            return;
        };
        for tab in saved {
            if let Some(t) = tab.table {
                if let Some(conn) = tab.connection_id {
                    self.pending_table_tabs.push((conn, t));
                }
                continue;
            }
            self.open_query_tab_with(&tab.sql, tab.file.clone(), window, cx);
            if let Some(Tab::Query {
                title,
                saved_text,
                file,
                connection_id,
                schema,
                row_limit,
                ..
            }) = self.state.tabs.last_mut()
            {
                *title = tab.title;
                *connection_id = tab.connection_id;
                *schema = tab.schema;
                *row_limit = tab.row_limit;
                // A file-backed tab is "dirty" if its text differs from what's on disk
                if let Some(disk) = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
                    *saved_text = disk;
                }
            }
        }
        if !self.state.tabs.is_empty() {
            self.state.active_tab_index = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_and_keeps_case() {
        assert_eq!(
            super::format_sql("select a,b from t where x=1"),
            "select\n  a,\n  b\nfrom\n  t\nwhere\n  x = 1"
        );
    }
}

/// Every SQLite 3 database starts with this 16-byte header
fn is_sqlite_db(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut header = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok()
        && &header == b"SQLite format 3\0"
}

#[cfg(test)]
mod sqlite_header_tests {
    #[test]
    fn detects_sqlite_header() {
        let dir = std::env::temp_dir();
        let db = dir.join("quarry-header-test.db");
        std::fs::write(&db, b"SQLite format 3\0rest-of-page").unwrap();
        let sql = dir.join("quarry-header-test.sql");
        std::fs::write(&sql, b"select 1;").unwrap();
        assert!(super::is_sqlite_db(&db));
        assert!(!super::is_sqlite_db(&sql));
    }
}
