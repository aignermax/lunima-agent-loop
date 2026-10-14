//! Shared core of the Lunima team agent (Product Owner, Coder, QA, PR feedback):
//! configuration, external tools (gh, git, claude), prompts and the state files the
//! Control Center reads. Native on Windows and Linux.

pub mod claude;
pub mod config;
pub mod gh;
pub mod git;
pub mod import;
pub mod paths;
pub mod proc;
pub mod prompts;
pub mod provider;
pub mod state;
