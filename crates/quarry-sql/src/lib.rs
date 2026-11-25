// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! # Quarry SQL (`quarry-sql`)
//!
//! SQL statement parsing, lexing, autocomplete, static analysis, parameter binding,
//! and AST-aware code actions for Quarry.
//!
//! ## Submodules
//!
//! - **[`splitter`]:** Semicolon and batch splitting respecting single/double quotes, dollar-quoting,
//!   C-style / line comments, SQL Server `GO` batches, Oracle PL/SQL `/` blocks, and Tree-sitter fallback.
//! - **[`completer`]:** Context-aware autocompletion engine resolving in-scope tables, CTEs, subquery projections,
//!   foreign key join conditions, intelligent table aliasing, fuzzy scoring with usage frequency ranking, and static schema linting.
//! - **[`params`]:** Parameter placeholder scanning (`:param`, `$1`) and literal binding with safe escaping.
//! - **[`rewrites`]:** Query assist actions including `*` expansion into catalog columns, automated `LIMIT` injection,
//!   and `EXPLAIN` wrapping.

pub mod completer;
pub mod params;
pub mod rewrites;
pub mod splitter;
pub mod stream;

pub use completer::{
    extract_ctes, extract_select_columns, find_usages, lint, match_score, mentioned_unloaded,
    rename_identifier, resolve_cte, resolve_qualifier, smart_alias, table_refs,
    CompletionCandidate, CompletionKind, CteSchema, SchemaCompleter, TableRef,
};
pub use params::{bind_params, find_params};
pub use rewrites::{rewrites, with_limit};
pub use splitter::{
    find_statement_at_offset, find_statement_at_offset_with, split_statements,
    split_statements_with, tree_sitter_split_statements, tree_sitter_statement_at_offset,
    SplitRules, StatementSpan,
};
pub use stream::StatementStream;
