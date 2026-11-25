// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Modal Vim state machine with operator-pending, text objects, registers, and dot-repeat.
//!
//! Inspired by Zed's `vim` crate (<https://github.com/zed-industries/zed>, GPL-3.0-or-later).

use super::buffer::Buffer;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    #[default]
    Normal,
    Insert,
    Visual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObject {
    InnerWord,
    InnerParens,
    InnerSingleQuote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedEdit {
    pub operator: Operator,
    pub object: TextObject,
}

/// Modal Vim state for an editor buffer.
#[derive(Debug, Default)]
pub struct VimEngine {
    pub mode: VimMode,
    pub pending_operator: Option<Operator>,
    pub register: String,
    pub last_edit: Option<RecordedEdit>,
}

impl VimEngine {
    /// Resolves a [`TextObject`] byte range around `cursor` in `buffer`.
    pub fn text_object_range(buffer: &Buffer, cursor: usize, obj: TextObject) -> Option<Range<usize>> {
        let s = buffer.as_str();
        let pos = cursor.min(s.len());
        match obj {
            TextObject::InnerWord => {
                let bytes = s.as_bytes();
                if pos >= bytes.len() || !is_word_byte(bytes[pos]) {
                    return None;
                }
                let mut start = pos;
                while start > 0 && is_word_byte(bytes[start - 1]) {
                    start -= 1;
                }
                let mut end = pos + 1;
                while end < bytes.len() && is_word_byte(bytes[end]) {
                    end += 1;
                }
                Some(start..end)
            }
            TextObject::InnerParens => delimited_range(s, pos, b'(', b')'),
            TextObject::InnerSingleQuote => delimited_range(s, pos, b'\'', b'\''),
        }
    }

    /// Applies `operator` over `object` at `cursor`, returning the new cursor offset.
    pub fn apply_operator(
        &mut self,
        buffer: &mut Buffer,
        cursor: usize,
        operator: Operator,
        object: TextObject,
    ) -> Option<usize> {
        let range = Self::text_object_range(buffer, cursor, object)?;
        self.register = buffer.as_str()[range.clone()].to_string();
        let new_cursor = match operator {
            Operator::Yank => cursor,
            Operator::Delete => {
                buffer.edit(range.clone(), "");
                self.last_edit = Some(RecordedEdit { operator, object });
                range.start
            }
            Operator::Change => {
                buffer.edit(range.clone(), "");
                self.mode = VimMode::Insert;
                self.last_edit = Some(RecordedEdit { operator, object });
                range.start
            }
        };
        self.pending_operator = None;
        Some(new_cursor)
    }

    /// Repeats the last recorded `Delete` or `Change` edit (`.` in Normal mode).
    pub fn repeat_last(&mut self, buffer: &mut Buffer, cursor: usize) -> Option<usize> {
        let edit = self.last_edit?;
        self.apply_operator(buffer, cursor, edit.operator, edit.object)
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn delimited_range(s: &str, pos: usize, open: u8, close: u8) -> Option<Range<usize>> {
    let bytes = s.as_bytes();
    let start = bytes[..=pos.min(bytes.len().saturating_sub(1))]
        .iter()
        .rposition(|&b| b == open)?;
    let rel_end = bytes[start + 1..].iter().position(|&b| b == close)?;
    Some((start + 1)..(start + 1 + rel_end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vim_delete_inner_word_and_dot_repeat() {
        let mut buf = Buffer::new("select alpha, beta from items;");
        let mut vim = VimEngine::default();
        let cur = vim
            .apply_operator(&mut buf, 8, Operator::Delete, TextObject::InnerWord)
            .unwrap();
        assert_eq!(buf.as_str(), "select , beta from items;");
        assert_eq!(vim.register, "alpha");
        assert_eq!(cur, 7);

        let cur2 = vim.repeat_last(&mut buf, 10).unwrap();
        assert_eq!(buf.as_str(), "select ,  from items;");
        assert_eq!(vim.register, "beta");
        assert_eq!(cur2, 9);
    }
}
