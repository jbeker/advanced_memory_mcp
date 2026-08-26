// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Jeremy Beker

//! Advanced Memory MCP — Rust implementation.
//!
//! Multi-user MCP memory server. Same MCP tool surface and JSONL data format
//! as the Python v0.2.x implementation (see benchmarks/fixtures/ for the
//! captured contract); storage is in-memory with atomic file persistence.

pub mod graph;
pub mod health;
pub mod mcp;
pub mod store;
pub mod tokens;
pub mod webui;
