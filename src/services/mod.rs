//! Service layer — business logic implementations.
//!
//! Services are the "Application Services" layer in the architecture.
//! They know nothing about MCP transport and are called by Tool implementations.

pub mod health;
pub mod stats;
