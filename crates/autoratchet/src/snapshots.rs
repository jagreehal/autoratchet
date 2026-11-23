//! Scores for every function, in the order they were measured, plus anything that could not be
//! measured. A non-empty `problems` list fails the gate: an unmeasured function is not evidence
//! that the function is fine.

use std::collections::HashMap;

use crate::gate::FunctionSnapshot;

/// Stable identity: relative file path plus qualified function name.
pub fn function_key(file_path: &str, function_name: &str) -> String {
    format!("{}::{function_name}", normalize_path(file_path))
}

pub fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

#[derive(Debug, Default)]
pub struct Snapshots {
    items: Vec<(String, FunctionSnapshot)>,
    index: HashMap<String, usize>,
    pub problems: Vec<String>,
}

impl Snapshots {
    /// Insert or replace, keeping the first position, like a JavaScript `Map`.
    pub fn insert(&mut self, snap: FunctionSnapshot) {
        let key = function_key(&snap.file_path, &snap.function_name);
        match self.index.get(&key) {
            Some(&i) => self.items[i].1 = snap,
            None => {
                self.index.insert(key.clone(), self.items.len());
                self.items.push((key, snap));
            }
        }
    }

    /// Put each under the key of the function it is compared with, or, for those `keys` gives
    /// none, under one no function has.
    pub fn rekey(&mut self, keys: &HashMap<String, Option<String>>) {
        for (key, _) in &mut self.items {
            match keys.get(key) {
                Some(Some(new)) => *key = new.clone(),
                Some(None) => key.push_str("\0gone"),
                None => {}
            }
        }
        let keys = self.items.iter().enumerate();
        self.index = keys.map(|(i, (key, _))| (key.clone(), i)).collect();
    }

    pub fn get(&self, key: &str) -> Option<&FunctionSnapshot> {
        self.index.get(key).map(|&i| &self.items[i].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut FunctionSnapshot> {
        self.index.get(key).map(|&i| &mut self.items[i].1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &FunctionSnapshot)> {
        self.items.iter().map(|(k, s)| (k.as_str(), s))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut FunctionSnapshot)> {
        self.items.iter_mut().map(|(k, s)| (k.as_str(), s))
    }

    pub fn values(&self) -> impl Iterator<Item = &FunctionSnapshot> {
        self.items.iter().map(|(_, s)| s)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
