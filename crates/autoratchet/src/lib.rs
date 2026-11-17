//! AutoRatchet: prevent code quality regressions, one pull request at a time.

pub mod collect;
pub mod compare;
pub mod config;
pub mod functions;
pub mod gate;
pub mod git;
pub mod go;
pub mod lang;
pub mod overrides;
pub mod python;
pub mod report;
pub mod rust;
pub mod snapshots;
pub mod tools;
