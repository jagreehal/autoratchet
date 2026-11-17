//! Complexity limits from the linters a repository already configures, so a team sets each limit
//! in one place: `.oxlintrc.json`, `biome.json`, Ruff, Clippy and golangci-lint.
//!
//! Only explicit numbers count. A rule switched on without one keeps AutoRatchet's default.
//! Config files are read at the root.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{Captures, Regex};

use serde_json::Value;

use crate::gate::Metric;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Js,
    Python,
    Rust,
    Go,
}

impl Lang {
    pub fn of(file: &str) -> Lang {
        if file.ends_with(".py") {
            Lang::Python
        } else if file.ends_with(".rs") {
            Lang::Rust
        } else if file.ends_with(".go") {
            Lang::Go
        } else {
            Lang::Js
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Limit {
    pub value: f64,
    /// The config file the limit came from, e.g. `ruff.toml`.
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits(HashMap<(Lang, Metric), Limit>);

impl Limits {
    pub fn get(&self, lang: Lang, metric: Metric) -> Option<&Limit> {
        self.0.get(&(lang, metric))
    }

    fn set(&mut self, lang: Lang, metric: Metric, value: f64, source: &str) {
        let stricter = self.get(lang, metric).is_none_or(|l| value < l.value);
        if stricter {
            self.0.insert(
                (lang, metric),
                Limit {
                    value,
                    source: source.into(),
                },
            );
        }
    }

    /// The lower limit of the two sides for each metric, so a change can tighten a limit at
    /// once but cannot loosen the one it is checked against.
    pub fn stricter(mut self, other: Limits) -> Limits {
        for ((lang, metric), limit) in other.0 {
            self.set(lang, metric, limit.value, &limit.source);
        }
        self
    }
}

/// One linter: the config files it reads, first match wins, and what it takes from them.
type Linter = (&'static [&'static str], fn(&Value, &str, &mut Limits));

const LINTERS: [Linter; 5] = [
    (&[".oxlintrc.json"], oxlint),
    (&["biome.json", "biome.jsonc"], biome),
    (&[".ruff.toml", "ruff.toml", "pyproject.toml"], ruff),
    (&["clippy.toml", ".clippy.toml"], clippy),
    (
        &[".golangci.yml", ".golangci.yaml", ".golangci.toml", ".golangci.json"],
        golangci,
    ),
];

/// Limits from the config files `read` finds. `read` takes a root-relative file name and
/// returns its text, or `None` when there is no such file.
pub fn read_limits(read: impl Fn(&str) -> Option<String>) -> Result<Limits, String> {
    let mut limits = Limits::default();
    for (names, apply) in LINTERS {
        if let Some((name, text)) = names.iter().find_map(|n| read(n).map(|t| (*n, t))) {
            apply(&parse(name, &text)?, name, &mut limits);
        }
    }
    Ok(limits)
}

fn oxlint(v: &Value, name: &str, limits: &mut Limits) {
    for rule in ["complexity", "eslint/complexity"] {
        if let Some(n) = rule_option(&v["rules"][rule], "max") {
            limits.set(Lang::Js, Metric::Cyclomatic, n, name);
        }
    }
}

fn biome(v: &Value, name: &str, limits: &mut Limits) {
    let rule = &v["linter"]["rules"]["complexity"]["noExcessiveCognitiveComplexity"];
    if rule["level"] != "off"
        && let Some(n) = number(&rule["options"]["maxAllowedComplexity"])
    {
        limits.set(Lang::Js, Metric::Cognitive, n, name);
    }
}

fn ruff(v: &Value, name: &str, limits: &mut Limits) {
    let ruff = if name == "pyproject.toml" {
        &v["tool"]["ruff"]
    } else {
        v
    };
    for mccabe in [&ruff["lint"]["mccabe"], &ruff["mccabe"]] {
        if let Some(n) = number(&mccabe["max-complexity"]) {
            limits.set(Lang::Python, Metric::Cyclomatic, n, name);
        }
    }
}

fn clippy(v: &Value, name: &str, limits: &mut Limits) {
    if let Some(n) = number(&v["cognitive-complexity-threshold"]) {
        limits.set(Lang::Rust, Metric::Cognitive, n, name);
    }
}

fn golangci(v: &Value, name: &str, limits: &mut Limits) {
    // v1 keeps settings under `linters-settings`, v2 under `linters.settings`.
    for settings in [&v["linters-settings"], &v["linters"]["settings"]] {
        for (linter, key, metric) in [
            ("gocyclo", "min-complexity", Metric::Cyclomatic),
            ("cyclop", "max-complexity", Metric::Cyclomatic),
            ("gocognit", "min-complexity", Metric::Cognitive),
        ] {
            if let Some(n) = number(&settings[linter][key]) {
                limits.set(Lang::Go, metric, n, name);
            }
        }
    }
}

fn parse(name: &str, text: &str) -> Result<Value, String> {
    let parsed = if name.ends_with(".toml") {
        toml::from_str(text).map_err(|e| e.to_string())
    } else if name.ends_with(".yml") || name.ends_with(".yaml") {
        serde_norway::from_str(text).map_err(|e| e.to_string())
    } else {
        serde_json::from_str(&strip_jsonc(text)).map_err(|e| e.to_string())
    };
    parsed.map_err(|e| format!("could not read the complexity limit in {name}: {e}"))
}

/// An ESLint-style rule's option, when the rule is on: `["warn", { "max": 8 }]`.
fn rule_option(rule: &Value, key: &str) -> Option<f64> {
    let on = !matches!(&rule[0], Value::String(s) if s == "off" || s == "allow") && rule[0] != 0;
    on.then(|| number(&rule[1][key])).flatten()
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite() && *n >= 1.0)
}

/// A JSON string, or a comment.
static STRING_OR_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""(?:\\.|[^"\\])*"|//[^\n]*|/\*[\s\S]*?\*/"#).unwrap());
/// A JSON string, or a comma before a closing bracket.
static STRING_OR_TRAILING_COMMA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""(?:\\.|[^"\\])*"|,(\s*[}\]])"#).unwrap());

/// JSON with comments and trailing commas, as Oxlint and Biome accept it, to plain JSON.
fn strip_jsonc(text: &str) -> String {
    let keep_strings = |m: &Captures| {
        if m[0].starts_with('"') {
            m[0].to_string()
        } else {
            String::new()
        }
    };
    let json = STRING_OR_COMMENT.replace_all(text, keep_strings);
    STRING_OR_TRAILING_COMMA
        .replace_all(&json, |m: &Captures| {
            m.get(1).map_or(m[0].to_string(), |close| close.as_str().to_string())
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(files: &[(&str, &str)]) -> Limits {
        read_limits(|name| files.iter().find(|(n, _)| *n == name).map(|(_, t)| t.to_string())).unwrap()
    }

    fn value(l: &Limits, lang: Lang, metric: Metric) -> Option<(f64, &str)> {
        l.get(lang, metric).map(|l| (l.value, l.source.as_str()))
    }

    #[test]
    fn reads_every_linter() {
        let l = limits(&[
            (
                ".oxlintrc.json",
                "{ // comment\n \"rules\": { \"complexity\": [\"warn\", { \"max\": 8, }], /* x */ },\n}",
            ),
            (
                "biome.json",
                r#"{"linter":{"rules":{"complexity":{"noExcessiveCognitiveComplexity":{"level":"error","options":{"maxAllowedComplexity":12}}}}}}"#,
            ),
            ("pyproject.toml", "[tool.ruff.lint.mccabe]\nmax-complexity = 7\n"),
            ("clippy.toml", "cognitive-complexity-threshold = 20\n"),
            (
                ".golangci.yml",
                "version: \"2\"\nlinters:\n  settings:\n    gocyclo:\n      min-complexity: 12\n    cyclop:\n      max-complexity: 9\n    gocognit:\n      min-complexity: 14\n",
            ),
        ]);
        assert_eq!(value(&l, Lang::Js, Metric::Cyclomatic), Some((8.0, ".oxlintrc.json")));
        assert_eq!(value(&l, Lang::Js, Metric::Cognitive), Some((12.0, "biome.json")));
        assert_eq!(
            value(&l, Lang::Python, Metric::Cyclomatic),
            Some((7.0, "pyproject.toml"))
        );
        assert_eq!(value(&l, Lang::Python, Metric::Cognitive), None);
        assert_eq!(value(&l, Lang::Rust, Metric::Cognitive), Some((20.0, "clippy.toml")));
        assert_eq!(value(&l, Lang::Go, Metric::Cyclomatic), Some((9.0, ".golangci.yml")));
        assert_eq!(value(&l, Lang::Go, Metric::Cognitive), Some((14.0, ".golangci.yml")));
    }

    #[test]
    fn ignores_rules_that_are_off_or_have_no_number() {
        let l = limits(&[
            (".oxlintrc.json", r#"{"rules":{"complexity":["off",{"max":3}]}}"#),
            ("ruff.toml", "[lint]\nselect = [\"C901\"]\n"),
            (
                ".golangci.yml",
                "linters-settings:\n  gocyclo:\n    min-complexity: 6\n",
            ),
        ]);
        assert_eq!(value(&l, Lang::Js, Metric::Cyclomatic), None);
        assert_eq!(value(&l, Lang::Python, Metric::Cyclomatic), None);
        assert_eq!(value(&l, Lang::Go, Metric::Cyclomatic), Some((6.0, ".golangci.yml")));
    }

    #[test]
    fn a_broken_config_is_an_error() {
        assert!(read_limits(|n| (n == "clippy.toml").then(|| "= nope".to_string())).is_err());
    }

    #[test]
    fn stricter_takes_the_lower_limit() {
        let base = limits(&[("ruff.toml", "[lint.mccabe]\nmax-complexity = 8\n")]);
        let looser = limits(&[("ruff.toml", "[lint.mccabe]\nmax-complexity = 20\n")]);
        let tighter = limits(&[("ruff.toml", "[lint.mccabe]\nmax-complexity = 5\n")]);
        assert_eq!(
            value(&base.clone().stricter(looser), Lang::Python, Metric::Cyclomatic)
                .unwrap()
                .0,
            8.0
        );
        assert_eq!(
            value(&base.stricter(tighter), Lang::Python, Metric::Cyclomatic)
                .unwrap()
                .0,
            5.0
        );
    }

    #[test]
    fn strips_comments_but_not_strings() {
        assert_eq!(
            strip_jsonc(r#"{"a": "// not a comment", "b": [1,],}"#),
            r#"{"a": "// not a comment", "b": [1]}"#
        );
        assert_eq!(
            strip_jsonc("{\"q\": \"say \\\"hi,}\\\"\", /* c */ \"n\": 1 // end\n}"),
            "{\"q\": \"say \\\"hi,}\\\"\",  \"n\": 1 \n}"
        );
    }
}
