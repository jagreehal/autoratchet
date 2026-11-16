//! Parse a JavaScript or TypeScript file once: every function's qualified name, keyed by the
//! UTF-8 byte offset Oxlint reports it at, plus parse errors and comment spans.
//!
//! Names come from the ESTree shape of the oxc AST, walked in field order:
//!   processPayment                 top-level function or `const fn = () => {}`
//!   PaymentService.validate        class method or class field
//!   handlers.onSubmit              method in an object literal assigned to a variable
//!   createChart>normalise          function declared inside another function
//!   render>items.map()             callback passed to a call
//!   PAYMENT_RULES["vip"]           table entry with a key; PAYMENT_RULES[3] without one
//!   default                        anonymous default export
//! A repeated name in one file gets #2, #3, … in source order.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use oxc::allocator::Allocator;
use oxc::parser::Parser;
use oxc::span::SourceType;
use serde_json::{Map, Value};

pub struct Analysis {
    /// Function start offset (UTF-8 bytes) → qualified name. Empty when the file does not parse.
    pub functions: HashMap<u32, String>,
    pub errors: Vec<String>,
    /// Starts of bodiless declarations (overload signatures, `declare function`, abstract
    /// methods). Oxlint scores them, but they hold no logic to measure.
    pub bodiless: HashSet<u32>,
    /// Function start offset → cognitive complexity, for every function Biome's
    /// `noExcessiveCognitiveComplexity` rule scores (getters and setters are not among them).
    pub cognitive: HashMap<u32, u8>,
    /// Comment spans, delimiters included.
    pub comments: Vec<(u32, u32)>,
}

pub fn analyse(path: &Path, source: &str) -> Analysis {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_default();
    // Oxlint reads JSX in .js files (common in React projects); so does the parser here.
    let source_type = if source_type.is_javascript() {
        source_type.with_jsx(true)
    } else {
        source_type
    };
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let comments = parsed
        .program
        .comments
        .iter()
        .map(|c| (c.span.start, c.span.end))
        .collect();
    if !parsed.diagnostics.is_empty() {
        let errors = parsed.diagnostics.iter().map(|e| e.message.to_string()).collect();
        return Analysis {
            functions: HashMap::new(),
            errors,
            bodiless: HashSet::new(),
            cognitive: HashMap::new(),
            comments,
        };
    }
    let json = parsed.program.to_estree_json(source_type.is_typescript(), false);
    // Minified bundles nest thousands of levels deep: no recursion limit (callers give the
    // thread a large stack), and a tree that cannot be read is a measurement problem.
    let json = replace_lone_surrogates(&json);
    let mut reader = serde_json::Deserializer::from_str(&json);
    reader.disable_recursion_limit();
    let program: Value = match serde::Deserialize::deserialize(&mut reader) {
        Ok(program) => program,
        Err(e) => {
            return Analysis {
                functions: HashMap::new(),
                errors: vec![format!("could not read its syntax tree: {e}")],
                bodiless: HashSet::new(),
                cognitive: HashMap::new(),
                comments,
            };
        }
    };
    let mut walker = Walker {
        source,
        found: HashMap::new(),
        seen: HashMap::new(),
        bodiless: HashSet::new(),
    };
    walker.visit(&program, None, &Context::default());
    let mut cognitive = Cognitive::default();
    cognitive.visit(&program, None);
    Analysis {
        cognitive: cognitive.scores,
        functions: walker.found,
        errors: Vec::new(),
        bodiless: walker.bodiless,
        comments,
    }
}

/// JavaScript strings may hold lone surrogates (`"\uD800"` in minified bundles); JSON parsers
/// reject them. Names never depend on them, so they become U+FFFD.
fn replace_lone_surrogates(json: &str) -> std::borrow::Cow<'_, str> {
    if !json.contains("\\ud") && !json.contains("\\uD") {
        return json.into();
    }
    let unit = |s: &str, i: usize| -> Option<u16> {
        let hex = s.get(i..i + 6)?.strip_prefix("\\u")?;
        u16::from_str_radix(hex, 16).ok()
    };
    let (bytes, mut out, mut i) = (json.as_bytes(), String::with_capacity(json.len()), 0);
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            let next = json[i..].find('\\').map_or(json.len(), |n| i + n);
            out.push_str(&json[i..next]);
            i = next;
            continue;
        }
        match unit(json, i) {
            Some(0xD800..=0xDBFF) if matches!(unit(json, i + 6), Some(0xDC00..=0xDFFF)) => {
                out.push_str(&json[i..i + 12]);
                i += 12;
            }
            Some(0xD800..=0xDFFF) => {
                out.push_str("\\ufffd");
                i += 6;
            }
            // Any other escape, `\\` included, is two characters or a full \uXXXX.
            Some(_) => {
                out.push_str(&json[i..i + 6]);
                i += 6;
            }
            None => {
                out.push_str(&json[i..(i + 2).min(json.len())]);
                i += 2;
            }
        }
    }
    out.into()
}

#[derive(Clone, Default)]
struct Context {
    /// Qualified name of the enclosing function.
    scope: Option<String>,
    /// Name of the enclosing class, when walking its body.
    class_name: Option<String>,
    /// Name of the variable an object or array literal is assigned to.
    owner_name: Option<String>,
    /// Inside a `['key', fn]` tuple: its one function takes the tuple's name, with no index.
    keyed_tuple: bool,
}

type Node = Map<String, Value>;

fn node(value: &Value) -> Option<&Node> {
    value
        .as_object()
        .filter(|o| o.get("type").is_some_and(Value::is_string))
}

fn kind(n: &Node) -> &str {
    n.get("type").and_then(Value::as_str).unwrap_or("")
}

fn offset(n: &Node, field: &str) -> usize {
    n.get(field).and_then(Value::as_u64).unwrap_or(0) as usize
}

fn is_function(n: &Node) -> bool {
    matches!(
        kind(n),
        "FunctionDeclaration" | "FunctionExpression" | "ArrowFunctionExpression"
    )
}

fn child<'a>(n: &'a Node, field: &str) -> Option<&'a Node> {
    n.get(field).and_then(node)
}

fn identifier_name(n: Option<&Node>) -> Option<String> {
    let n = n?;
    (kind(n) == "Identifier").then(|| n.get("name").and_then(Value::as_str).unwrap_or("").to_string())
}

fn string_literal(n: Option<&Node>) -> Option<String> {
    let n = n?;
    if kind(n) != "Literal" {
        return None;
    }
    n.get("value").and_then(Value::as_str).map(str::to_string)
}

/// `String(value)` for a JSON literal value, as JavaScript would print it.
fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

const KEY_PROPERTIES: [&str; 4] = ["id", "key", "name", "outcome"];

/// Node types an owner name passes through on its way to a literal's entries.
const KEEPS_OWNER: [&str; 5] = [
    "ObjectExpression",
    "ArrayExpression",
    "Property",
    "TSAsExpression",
    "TSSatisfiesExpression",
];

struct Walker<'s> {
    source: &'s str,
    found: HashMap<u32, String>,
    seen: HashMap<String, u32>,
    bodiless: HashSet<u32>,
}

impl Walker<'_> {
    fn text(&self, n: &Node) -> &str {
        self.source.get(offset(n, "start")..offset(n, "end")).unwrap_or("")
    }

    fn key_name(&self, key: Option<&Node>) -> String {
        let Some(key) = key else { return "?".into() };
        match kind(key) {
            "Identifier" => identifier_name(Some(key)).unwrap_or_default(),
            "PrivateIdentifier" => format!("#{}", key.get("name").and_then(Value::as_str).unwrap_or("")),
            "Literal" => js_string(key.get("value").unwrap_or(&Value::Null)),
            _ => format!("[{}]", self.text(key)),
        }
    }

    fn snippet(&self, n: &Node) -> String {
        let text = self.text(n).split_whitespace().collect::<Vec<_>>().join(" ");
        // JavaScript measures string length in UTF-16 units.
        if text.encode_utf16().count() <= 40 {
            return text;
        }
        let mut units = 0;
        let cut: String = text
            .chars()
            .take_while(|c| {
                units += c.len_utf16();
                units <= 37
            })
            .collect();
        format!("{cut}...")
    }

    /// `items.map((i) => i).filter` → `items.map().filter`: call chains without their arguments.
    fn callee_name(&self, n: &Node) -> String {
        match kind(n) {
            "Identifier" => identifier_name(Some(n)).unwrap_or_default(),
            "ThisExpression" => "this".into(),
            "MemberExpression" => {
                let Some(object) = child(n, "object") else {
                    return self.snippet(n);
                };
                let property = if n.get("computed").and_then(Value::as_bool).unwrap_or(false) {
                    format!(
                        "[{}]",
                        child(n, "property").map(|p| self.snippet(p)).unwrap_or_default()
                    )
                } else {
                    format!(".{}", self.key_name(child(n, "property")))
                };
                self.callee_name(object) + &property
            }
            "CallExpression" => match child(n, "callee") {
                Some(callee) => format!("{}()", self.callee_name(callee)),
                None => self.snippet(n),
            },
            _ => self.snippet(n),
        }
    }

    /// How to name an anonymous function, by the syntax it sits in.
    fn by_parent(&self, func: &Node, parent: &Node, ctx: &Context) -> Option<String> {
        let member = || {
            format!(
                "{}.{}",
                ctx.class_name.as_deref().unwrap_or("class"),
                self.key_name(child(parent, "key"))
            )
        };
        let callback = || child(parent, "callee").map_or("callback".into(), |c| format!("{}()", self.callee_name(c)));
        Some(match kind(parent) {
            "VariableDeclarator" => identifier_name(child(parent, "id")).unwrap_or_else(|| "anonymous".into()),
            "MethodDefinition" | "PropertyDefinition" | "AccessorProperty" => member(),
            "Property" => match &ctx.owner_name {
                Some(owner) => format!("{owner}.{}", self.key_name(child(parent, "key"))),
                None => self.key_name(child(parent, "key")),
            },
            "AssignmentExpression" => child(parent, "left").map_or("anonymous".into(), |l| self.snippet(l)),
            "CallExpression" | "NewExpression" => callback(),
            "ArrayExpression" => self.array_entry_name(func, parent, ctx),
            "ExportDefaultDeclaration" => "default".into(),
            _ => return None,
        })
    }

    fn array_entry_name(&self, func: &Node, parent: &Node, ctx: &Context) -> String {
        let elements = parent
            .get("elements")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let functions = elements.iter().filter(|e| node(e).is_some_and(is_function)).count();
        if let (true, Some(owner), 1) = (ctx.keyed_tuple, &ctx.owner_name, functions) {
            return owner.clone();
        }
        let index = elements
            .iter()
            .position(|e| e.as_object().is_some_and(|o| std::ptr::eq(o, func)));
        format!(
            "{}[{}]",
            ctx.owner_name.as_deref().unwrap_or("array"),
            index.map_or(-1, |i| i as i64)
        )
    }

    fn own_name(&self, func: &Node, parent: Option<&Node>, ctx: &Context) -> String {
        identifier_name(child(func, "id"))
            .or_else(|| parent.and_then(|p| self.by_parent(func, p, ctx)))
            .unwrap_or_else(|| "anonymous".into())
    }

    /// Class members get no scope prefix: the class name already carries it.
    fn record(&mut self, func: &Node, parent: Option<&Node>, ctx: &Context) -> String {
        let own = self.own_name(func, parent, ctx);
        self.record_as(func, own, ctx)
    }

    fn record_as(&mut self, func: &Node, own: String, ctx: &Context) -> String {
        let base = match &ctx.scope {
            Some(scope) => format!("{scope}>{own}"),
            None => own,
        };
        let count = self.seen.entry(base.clone()).or_insert(0);
        *count += 1;
        let name = if *count == 1 { base } else { format!("{base}#{count}") };
        self.found.insert(offset(func, "start") as u32, name.clone());
        name
    }

    fn class_context(&self, n: &Node, parent: Option<&Node>, ctx: &Context) -> Context {
        let declared_as = parent
            .filter(|p| kind(p) == "VariableDeclarator")
            .and_then(|p| identifier_name(child(p, "id")));
        let name = identifier_name(child(n, "id"))
            .or(declared_as)
            .unwrap_or_else(|| "class".into());
        match &ctx.scope {
            Some(scope) => Context {
                class_name: Some(format!("{scope}>{name}")),
                ..Context::default()
            },
            None => Context {
                class_name: Some(name),
                ..ctx.clone()
            },
        }
    }

    /// The context a non-function node hands to its children.
    fn child_context(&self, n: &Node, parent: Option<&Node>, ctx: &Context) -> Context {
        match kind(n) {
            "ClassDeclaration" | "ClassExpression" => self.class_context(n, parent, ctx),
            "VariableDeclarator" => match identifier_name(child(n, "id")) {
                Some(id) => Context {
                    owner_name: Some(id),
                    ..ctx.clone()
                },
                None => ctx.clone(),
            },
            "Property" => {
                let nested_object = child(n, "value").is_some_and(|v| kind(v) == "ObjectExpression");
                match (&ctx.owner_name, nested_object) {
                    (Some(owner), true) => Context {
                        owner_name: Some(format!("{owner}.{}", self.key_name(child(n, "key")))),
                        ..ctx.clone()
                    },
                    _ => ctx.clone(),
                }
            }
            // An owner name only reaches literals directly under the declaration.
            k if ctx.owner_name.is_some() && !KEEPS_OWNER.contains(&k) => Context {
                owner_name: None,
                ..ctx.clone()
            },
            _ => ctx.clone(),
        }
    }

    /// Entries are named by their key when they have one, so reordering a table cannot pair a
    /// rule with another rule's history. Entries without a key fall back to position.
    fn entry_context(&self, item: &Node, index: usize, owner: &str, ctx: &Context) -> Context {
        let is_array = kind(item) == "ArrayExpression";
        if !is_array && kind(item) != "ObjectExpression" {
            return ctx.clone();
        }
        match entry_key(item) {
            None => Context {
                owner_name: Some(format!("{owner}[{index}]")),
                ..ctx.clone()
            },
            Some(key) => Context {
                owner_name: Some(format!("{owner}[{}]", Value::String(key))),
                keyed_tuple: is_array,
                ..ctx.clone()
            },
        }
    }

    fn visit(&mut self, value: &Value, parent: Option<&Node>, ctx: &Context) {
        let Some(n) = node(value) else { return };
        if matches!(kind(n), "TSDeclareFunction" | "TSEmptyBodyFunctionExpression") {
            self.bodiless.insert(offset(n, "start") as u32);
        }
        let child_ctx = if is_function(n) {
            Context {
                scope: Some(self.record(n, parent, ctx)),
                ..Context::default()
            }
        } else if kind(n) == "StaticBlock" {
            // `static { … }` holds logic Oxlint scores like a function's: Class.static.
            let own = format!("{}.static", ctx.class_name.as_deref().unwrap_or("class"));
            Context {
                scope: Some(self.record_as(n, own, &Context::default())),
                ..Context::default()
            }
        } else {
            self.child_context(n, parent, ctx)
        };
        let owner = if kind(n) == "ArrayExpression" {
            ctx.owner_name.clone()
        } else {
            None
        };
        match owner {
            None => {
                for (field, v) in n {
                    if matches!(field.as_str(), "type" | "start" | "end") {
                        continue;
                    }
                    match v {
                        Value::Array(items) => items.iter().for_each(|item| self.visit(item, Some(n), &child_ctx)),
                        _ => self.visit(v, Some(n), &child_ctx),
                    }
                }
            }
            // Real indexes, holes included, so `[a, , b]` names b as [2].
            Some(owner) => {
                let elements = n
                    .get("elements")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                for (i, item) in elements.iter().enumerate() {
                    if let Some(entry) = node(item) {
                        let entry_ctx = self.entry_context(entry, i, &owner, &child_ctx);
                        self.visit(item, Some(n), &entry_ctx);
                    }
                }
            }
        }
    }
}

/// Cognitive complexity, computed the way Biome 2.6's `noExcessiveCognitiveComplexity` rule does
/// (after SonarSource's paper), so scores match what Biome reports. One pass, in source order:
/// - `if`, `?:`, `switch`, loops and `catch` cost 1 plus the current nesting, and nest.
/// - `else` costs 1; `else if` costs like an `if` one level out.
/// - `finally`, `with` and labelled `break`/`continue` cost 1.
/// - Each run of the same logical operator (`&&`, `||`, `??`) costs 1.
/// - A nested function starts one level deeper than the code around it and scores separately.
#[derive(Default)]
struct Cognitive {
    stack: Vec<CognitiveState>,
    scores: HashMap<u32, u8>,
}

struct CognitiveState {
    function: *const Node,
    score: u8,
    nesting: u8,
}

const MAX_SCORE: u8 = u8::MAX;
const MAX_FUNCTION_DEPTH: usize = 10;

fn nests(n: &Node) -> bool {
    matches!(
        kind(n),
        "IfStatement"
            | "ConditionalExpression"
            | "SwitchStatement"
            | "CatchClause"
            | "ForStatement"
            | "ForInStatement"
            | "ForOfStatement"
            | "WhileStatement"
            | "DoWhileStatement"
    )
}

/// Biome scores functions, arrows, methods and constructors; getters and setters belong to the
/// function around them.
fn is_cognitive_function(n: &Node, parent: Option<&Node>) -> bool {
    match kind(n) {
        "FunctionDeclaration" | "ArrowFunctionExpression" => true,
        "FunctionExpression" => !parent.is_some_and(|p| {
            matches!(kind(p), "MethodDefinition" | "Property")
                && matches!(p.get("kind").and_then(Value::as_str), Some("get" | "set"))
        }),
        _ => false,
    }
}

fn operator(n: &Node) -> Option<&str> {
    n.get("operator").and_then(Value::as_str)
}

impl Cognitive {
    fn active(&mut self) -> Option<&mut CognitiveState> {
        self.stack.last_mut().filter(|s| s.score < MAX_SCORE)
    }

    fn enter(&mut self, n: &Node, parent: Option<&Node>) {
        if self.stack.last().is_some_and(|s| s.score == MAX_SCORE) {
            return;
        }
        if is_cognitive_function(n, parent) {
            let nesting = self.stack.last().map_or(0, |s| s.nesting.saturating_add(1));
            if self.stack.len() < MAX_FUNCTION_DEPTH {
                self.stack.push(CognitiveState {
                    function: n,
                    score: 0,
                    nesting,
                });
            } else if let Some(top) = self.stack.last_mut() {
                top.score = MAX_SCORE;
            }
        }
        let Some(state) = self.stack.last_mut() else { return };
        let labelled_jump =
            matches!(kind(n), "BreakStatement" | "ContinueStatement") && n.get("label").is_some_and(|l| !l.is_null());
        if nests(n) {
            state.score = state.score.saturating_add(1).saturating_add(state.nesting);
            state.nesting = state.nesting.saturating_add(1);
        } else if kind(n) == "WithStatement" || labelled_jump {
            state.score = state.score.saturating_add(1);
        } else if kind(n) == "LogicalExpression" {
            // A chain `a && b && c` parses as `(a && b) && c`: only its outermost link costs 1.
            let continues_chain = parent.is_some_and(|p| {
                kind(p) == "LogicalExpression"
                    && operator(p) == operator(n)
                    && p.get("left")
                        .and_then(Value::as_object)
                        .is_some_and(|l| std::ptr::eq(l, n))
            });
            if !continues_chain {
                state.score = state.score.saturating_add(1);
            }
        }
    }

    fn leave(&mut self, n: &Node, parent: Option<&Node>) {
        if is_cognitive_function(n, parent) {
            if let Some(done) = self.stack.pop_if(|s| std::ptr::eq(s.function, n)) {
                self.scores.insert(offset(n, "start") as u32, done.score);
            }
        } else if nests(n)
            && let Some(state) = self.active()
        {
            state.nesting = state.nesting.saturating_sub(1);
        }
    }

    /// `else`: Biome's else clause node, which plain ESTree does not have.
    fn else_clause(&mut self, alternate: &Node, parent: &Node) {
        let is_else_if = kind(alternate) == "IfStatement";
        if let Some(state) = self.active() {
            if is_else_if {
                state.nesting = state.nesting.saturating_sub(1);
            } else {
                state.score = state.score.saturating_add(1);
            }
        }
        self.visit_node(alternate, Some(parent));
        if let Some(state) = self.active() {
            state.nesting = if is_else_if {
                state.nesting.saturating_add(1)
            } else {
                state.nesting.saturating_sub(1)
            };
        }
    }

    fn visit(&mut self, value: &Value, parent: Option<&Node>) {
        match value {
            Value::Array(items) => items.iter().for_each(|item| self.visit(item, parent)),
            _ => {
                if let Some(n) = node(value) {
                    self.visit_node(n, parent);
                }
            }
        }
    }

    fn visit_node(&mut self, n: &Node, parent: Option<&Node>) {
        self.enter(n, parent);
        for (field, v) in n {
            match (kind(n), field.as_str()) {
                (_, "type" | "start" | "end") => {}
                ("IfStatement", "alternate") if node(v).is_some() => self.else_clause(node(v).expect("checked"), n),
                ("TryStatement", "finalizer") if node(v).is_some() => {
                    // `finally` costs 1, without nesting.
                    if let Some(state) = self.stack.last_mut().filter(|s| s.score < MAX_SCORE) {
                        state.score = state.score.saturating_add(1);
                    }
                    self.visit(v, Some(n));
                }
                _ => self.visit(v, Some(n)),
            }
        }
        self.leave(n, parent);
    }
}

/// A key that names a table entry regardless of its position, if the entry has one.
fn entry_key(item: &Node) -> Option<String> {
    match kind(item) {
        "ArrayExpression" => string_literal(item.get("elements")?.as_array()?.first().and_then(node)),
        "ObjectExpression" => item
            .get("properties")?
            .as_array()?
            .iter()
            .filter_map(node)
            .find_map(|p| {
                let is_key = kind(p) == "Property"
                    && identifier_name(child(p, "key")).is_some_and(|k| KEY_PROPERTIES.contains(&k.as_str()));
                if is_key {
                    string_literal(child(p, "value"))
                } else {
                    None
                }
            }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<String> {
        let mut found: Vec<_> = analyse(Path::new("a.ts"), source).functions.into_iter().collect();
        found.sort();
        found.into_iter().map(|(_, n)| n).collect()
    }

    #[test]
    fn qualified_names() {
        assert_eq!(
            names(
                "class A { m() { const f = () => 1; } static { const g = () => 2; } }\nconst o = { k() {} };\nconst R = [['x', () => 1], () => 2];"
            ),
            ["A.m", "A.m>f", "A.static", "A.static>g", "o.k", "R[\"x\"]", "R[1]"]
        );
    }

    fn cognitive(source: &str) -> Vec<(String, u8)> {
        let a = analyse(Path::new("a.ts"), source);
        let mut out: Vec<_> = a
            .functions
            .iter()
            .map(|(start, name)| (name.clone(), a.cognitive.get(start).copied().unwrap_or(0)))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn cognitive_scores() {
        // Each case checked against Biome 2.6.7.
        let src = "function f(x) { for (;;) { for (;;) { for (;;) { if (x) { if (x) { g(x > 1 ? 1 : 2); } } } } } }";
        assert_eq!(cognitive(src), [("f".into(), 21)]);
        assert_eq!(
            cognitive("function f(a, b, c) { return a && b && c || a; }"),
            [("f".into(), 2)]
        );
        assert_eq!(
            cognitive("function f(a) { if (a) {} else if (!a) {} else {} }"),
            [("f".into(), 3)]
        );
        assert_eq!(
            cognitive("function f(a) { if (a) { const g = () => a ? 1 : 2; } }"),
            [("f".into(), 1), ("f>g".into(), 3)]
        );
        assert_eq!(
            cognitive("function f() { try {} catch { } finally {} }"),
            [("f".into(), 2)]
        );
    }

    #[test]
    fn jsx_in_js_files() {
        let found = analyse(
            Path::new("a.js"),
            "export default function A(a) { return a ? <div>{1}</div> : null; }",
        );
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        assert_eq!(found.functions.values().collect::<Vec<_>>(), ["A"]);
    }

    #[test]
    fn lone_surrogates() {
        assert_eq!(
            replace_lone_surrogates(r#""\uD800" "\uD83D\uDE00" "\\uD800""#),
            r#""\ufffd" "\uD83D\uDE00" "\\uD800""#
        );
        assert_eq!(names("const s = '\\uD800'; function f() {}"), ["f"]);
    }

    #[test]
    fn offsets_are_utf8_bytes() {
        let found = analyse(Path::new("a.ts"), "const s = 'é';\nfunction f() {}").functions;
        assert_eq!(found.get(&16).map(String::as_str), Some("f"));
    }
}
