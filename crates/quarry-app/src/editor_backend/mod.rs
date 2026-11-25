// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Virtualized, Zed-inspired editor backend for Quarry.
//!
//! Architecture (SumTree-backed rope, layered DisplayMap, and incremental Tree-sitter)
//! is inspired by Zed (<https://github.com/zed-industries/zed>, GPL-3.0-or-later).

pub mod buffer;
pub mod display_map;
pub mod gutter;
pub mod view;
pub mod vim_state;

pub use buffer::{Buffer, Point, TextSummary};
pub use display_map::{DisplayMap, DisplayRow};
pub use gutter::StatementIndex;
pub use view::{QuarryEditorEvent, QuarryEditorView};
pub use vim_state::{Operator, TextObject, VimEngine, VimMode};
