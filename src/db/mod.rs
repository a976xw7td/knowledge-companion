//! Database layer — SQLite connection, migrations, and repository access.
//!
//! All database access goes through this module. Migrations are embedded
//! at compile time via `include_str!` — no runtime SQL files needed.

pub mod connection;
pub mod migrations;
