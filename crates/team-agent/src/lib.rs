//! The Lunima agent team as a library: roles (coder, QA, PR feedback), the shared pieces
//! they use, and the daemon that runs them. `main.rs` is the command-line front end.

pub mod agent_toml;
pub mod claim;
pub mod coder;
pub mod ctx;
pub mod directives;
pub mod forge;
pub mod llm;
pub mod log;
pub mod pr_text;
pub mod repos;
pub mod review;
pub mod role;
pub mod session;
pub mod shell;
pub mod walkthrough;
pub mod workspace;

#[cfg(test)]
pub mod fake;
