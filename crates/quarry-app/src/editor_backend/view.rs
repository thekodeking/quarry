// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! GPUI view wrapping [`Buffer`], [`DisplayMap`], [`StatementIndex`], and [`VimEngine`].
//!
//! Renders only the visible viewport rows with a line-number gutter, vertical/horizontal
//! scrolling, clipboard/undo/redo, and fast viewport-only SQL token highlighting.

use super::{Buffer, DisplayMap, Point, StatementIndex, VimEngine};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::ops::Range;

/// Events emitted by [`QuarryEditorView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarryEditorEvent {
    Change,
}

/// Virtualized SQL editor view backed by [`Buffer`] and [`DisplayMap`].
pub struct QuarryEditorView {
    pub buffer: Buffer,
    pub display_map: DisplayMap,
    pub statement_index: StatementIndex,
    pub vim: VimEngine,
    pub cursor: usize,
    pub selection: Option<Range<usize>>,
    pub soft_wrap: bool,
    pub line_numbers: bool,
    pub line_height: Pixels,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub focus_handle: FocusHandle,
}

impl EventEmitter<QuarryEditorEvent> for QuarryEditorView {}

impl Focusable for QuarryEditorView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl QuarryEditorView {
    pub fn new(initial: &str, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            buffer: Buffer::new(initial),
            display_map: DisplayMap::new(None),
            statement_index: StatementIndex::default(),
            vim: VimEngine::default(),
            cursor: 0,
            selection: None,
            soft_wrap: false,
            line_numbers: true,
            line_height: px(20.0),
            scroll_x: 0.0,
            scroll_y: 0.0,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn value(&self) -> SharedString {
        self.buffer.as_str().to_string().into()
    }

    pub fn set_value(&mut self, text: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let len = self.buffer.len();
        self.buffer.edit(0..len, text);
        self.cursor = self.cursor.min(self.buffer.len());
        self.selection = None;
        cx.emit(QuarryEditorEvent::Change);
        cx.notify();
    }

    pub fn cursor(&self) -> usize {
        self.cursor.min(self.buffer.len())
    }

    pub fn selected_range(&self) -> Range<usize> {
        self.selection
            .clone()
            .unwrap_or_else(|| self.cursor()..self.cursor())
    }

    pub fn set_selected_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let start = range.start.min(self.buffer.len());
        let end = range.end.min(self.buffer.len()).max(start);
        self.cursor = end;
        self.selection = if start < end { Some(start..end) } else { None };
        cx.notify();
    }

    pub fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = range.start.min(self.buffer.len());
        self.buffer.edit(range, text);
        self.cursor = start + text.len();
        self.selection = None;
        cx.emit(QuarryEditorEvent::Change);
        cx.notify();
    }

    pub fn set_soft_wrap(&mut self, wrap: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let effective = wrap && self.buffer.len() <= 512 * 1024;
        if self.soft_wrap != effective {
            self.soft_wrap = effective;
            self.display_map.wrap_columns = if effective { Some(100) } else { None };
            cx.notify();
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }

    pub fn line_height(&self) -> Option<Pixels> {
        Some(self.line_height)
    }

    pub fn scroll_offset(&self) -> gpui::Point<Pixels> {
        point(px(-self.scroll_x), px(-self.scroll_y))
    }

    pub fn input_bounds(&self) -> Bounds<Pixels> {
        Bounds::default()
    }

    pub fn cursor_layout(&self) -> Option<(Bounds<Pixels>, usize)> {
        None
    }

    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let range = self
            .selection
            .take()
            .unwrap_or_else(|| self.cursor..self.cursor);
        let start = range.start.min(self.buffer.len());
        self.buffer.edit(range, text);
        self.cursor = start + text.len();
        cx.emit(QuarryEditorEvent::Change);
        cx.notify();
    }

    pub fn backspace(&mut self, cx: &mut Context<Self>) {
        if let Some(sel) = self.selection.take() {
            self.buffer.edit(sel.clone(), "");
            self.cursor = sel.start;
            cx.emit(QuarryEditorEvent::Change);
            cx.notify();
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let mut prev = self.cursor - 1;
        while prev > 0 && !self.buffer.as_str().is_char_boundary(prev) {
            prev -= 1;
        }
        self.buffer.edit(prev..self.cursor, "");
        self.cursor = prev;
        cx.emit(QuarryEditorEvent::Change);
        cx.notify();
    }

    pub fn move_cursor(&mut self, key: &str, cx: &mut Context<Self>) {
        let pt = self.buffer.offset_to_point(self.cursor());
        let next = match key {
            "left" => {
                let mut p = self.cursor();
                if p > 0 {
                    p -= 1;
                    while p > 0 && !self.buffer.as_str().is_char_boundary(p) {
                        p -= 1;
                    }
                }
                p
            }
            "right" => {
                let s = self.buffer.as_str();
                let mut p = self.cursor();
                if p < s.len() {
                    p += 1;
                    while p < s.len() && !s.is_char_boundary(p) {
                        p += 1;
                    }
                }
                p
            }
            "up" => self
                .buffer
                .point_to_offset(Point::new(pt.row.saturating_sub(1), pt.column)),
            "down" => self
                .buffer
                .point_to_offset(Point::new(pt.row + 1, pt.column)),
            "home" => self.buffer.point_to_offset(Point::new(pt.row, 0)),
            "end" => self
                .buffer
                .point_to_offset(Point::new(pt.row, usize::MAX / 2)),
            _ => self.cursor(),
        };
        self.cursor = next;
        self.selection = None;
        cx.notify();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(pos) = self.buffer.undo() {
            self.cursor = pos;
            self.selection = None;
            cx.emit(QuarryEditorEvent::Change);
            cx.notify();
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(pos) = self.buffer.redo() {
            self.cursor = pos;
            self.selection = None;
            cx.emit(QuarryEditorEvent::Change);
            cx.notify();
        }
    }
}

impl Render for QuarryEditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lh: f32 = self.line_height.into();
        let total_lines = self.buffer.line_count();
        let max_scroll_y = ((total_lines as f32) * lh - 120.0).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll_y);

        let visible = self
            .display_map
            .visible_buffer_range(self.scroll_y, 700.0, lh, total_lines);
        let rows = self.display_map.rows_for_range(&self.buffer, visible);
        let cursor_pt: Point = self.buffer.offset_to_point(self.cursor());
        let gutter_digits = total_lines.max(1).to_string().len().max(2);
        let gutter_w = px((gutter_digits as f32) * 8.5 + 20.0);

        div()
            .id("quarry-editor-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_row()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let delta = ev.delta.pixel_delta(this.line_height);
                let dy: f32 = delta.y.into();
                let dx: f32 = delta.x.into();
                this.scroll_y = (this.scroll_y - dy).max(0.0);
                this.scroll_x = (this.scroll_x - dx).max(0.0);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                let m = &ev.keystroke.modifiers;
                if m.platform && ev.keystroke.key == "v" {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        this.insert_text(&text, cx);
                    }
                    return;
                }
                if m.platform && ev.keystroke.key == "a" {
                    this.set_selected_range(0..this.buffer.len(), cx);
                    return;
                }
                if m.platform && ev.keystroke.key == "c" {
                    if let Some(sel) = &this.selection {
                        let text = this.buffer.as_str()[sel.clone()].to_string();
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    return;
                }
                if m.platform && ev.keystroke.key == "z" {
                    if m.shift {
                        this.redo(cx);
                    } else {
                        this.undo(cx);
                    }
                    return;
                }
                match ev.keystroke.key.as_str() {
                    "backspace" => this.backspace(cx),
                    "enter" => this.insert_text("\n", cx),
                    "tab" => this.insert_text("  ", cx),
                    "left" | "right" | "up" | "down" | "home" | "end" => {
                        this.move_cursor(&ev.keystroke.key, cx);
                    }
                    _ => {
                        if let Some(ch) = &ev.keystroke.key_char {
                            if !m.platform && !m.control {
                                this.insert_text(ch, cx);
                            }
                        }
                    }
                }
            }))
            .when(self.line_numbers, |d| {
                d.child(
                    div()
                        .w(gutter_w)
                        .h_full()
                        .flex_shrink_0()
                        .border_r_1()
                        .border_color(rgba(0xffffff14))
                        .flex()
                        .flex_col()
                        .children(rows.iter().map(|r| {
                            let is_active = !r.is_continuation && r.buffer_row == cursor_pt.row;
                            div()
                                .h(self.line_height)
                                .pr_2()
                                .flex()
                                .items_center()
                                .justify_end()
                                .text_xs()
                                .text_color(if is_active {
                                    rgba(0xe6e7eaff)
                                } else {
                                    rgba(0x7b7e86ff)
                                })
                                .child(if r.is_continuation {
                                    String::new()
                                } else {
                                    (r.buffer_row + 1).to_string()
                                })
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .children(rows.into_iter().map(|r| {
                        let is_cursor_row = !r.is_continuation && r.buffer_row == cursor_pt.row;
                        div()
                            .h(self.line_height)
                            .pl(px(8.0 - self.scroll_x))
                            .pr_2()
                            .flex()
                            .items_center()
                            .whitespace_nowrap()
                            .when(is_cursor_row, |d| d.bg(rgba(0xffffff0a)))
                            .child(r.text.to_string())
                    })),
            )
    }
}
