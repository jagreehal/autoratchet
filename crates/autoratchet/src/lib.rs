//! AutoRatchet: prevent code quality regressions, one pull request at a time.

pub mod collect;
pub mod compare;
pub mod functions;
pub mod gate;
pub mod git;
pub mod overrides;
pub mod report;
pub mod snapshots;
pub mod tools;
