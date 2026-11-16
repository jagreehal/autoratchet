//! Find the Oxlint executable that measures cyclomatic complexity.
//!
//! Order: `AUTORATCHET_OXLINT`, then `node_modules/.bin` in the current directory or any parent,
//! then `PATH`. With `AUTORATCHET_NODE` set, Oxlint runs as a script under that Node.js: the npm
//! package uses this, because npm ships Oxlint as a Node.js module.

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Tools {
    /// Needed only to measure JavaScript and TypeScript: the reason it is missing otherwise.
    pub oxlint: Result<PathBuf, String>,
    pub node: Option<PathBuf>,
}

impl Tools {
    pub fn discover() -> Result<Tools, String> {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        Ok(Tools {
            oxlint: find("oxlint", "AUTORATCHET_OXLINT", &cwd),
            node: std::env::var_os("AUTORATCHET_NODE")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
        })
    }

    pub fn command(&self, tool: &Path) -> Command {
        match &self.node {
            Some(node) => {
                let mut cmd = Command::new(node);
                cmd.arg(tool);
                cmd
            }
            None => Command::new(tool),
        }
    }
}

fn find(name: &str, env: &str, cwd: &Path) -> Result<PathBuf, String> {
    // Absolute, because Oxlint runs from a temporary directory.
    if let Some(path) = std::env::var_os(env).filter(|p| !p.is_empty()) {
        return std::path::absolute(&path).map_err(|e| e.to_string());
    }
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let in_node_modules = cwd.ancestors().map(|dir| dir.join("node_modules/.bin").join(&exe));
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|dir| dir.join(&exe));
    in_node_modules
        .chain(on_path)
        .find(|p| p.is_file())
        .ok_or_else(|| format!("could not find {name}. Install it (npm install -D {name}) or set {env} to its path."))
}
