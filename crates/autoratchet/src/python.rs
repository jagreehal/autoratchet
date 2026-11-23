//! Python: functions and methods, their cyclomatic and cognitive complexity, and override comments,
//! from one tree-sitter parse.
//!
//! The functions are the ones complexipy reports: top-level functions and the methods of classes,
//! including those defined inside `if`, `try` and the like. A function defined inside another one
//! is part of it, as in complexipy. Names use dots: `parse`, `Order.total`, `Outer.Inner.method`,
//! and `Order.total.setter` for a property's setter.
//!
//! Cyclomatic complexity follows radon: 1, plus each `if`, `elif` and conditional expression; each
//! `for` and `while`, and their `else`; each `except` and a `try`'s `else`; each `and` and `or`;
//! each comprehension `for` and `if`; each `match` case but a catch-all; and each `assert`, whose
//! condition radon does not look into. The
//! decisions in nested functions count toward the function around them. A class defined inside a
//! function is left out of its cyclomatic score, and its methods are not reported, as in radon
//! (complexipy counts that code toward the function's cognitive score). `@overload` signatures
//! are not functions to measure.
//!
//! Cognitive complexity follows complexipy (SonarSource's specification): `if`, loops, `except`,
//! `match` and conditional expressions cost 1 plus their nesting; `elif` and `else` cost 1; each
//! run of one boolean operator costs 1; a nested function's body sits one level deeper, unless the
//! function is a decorator's wrapper; and a function that calls itself costs 1 more.

use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser};

use crate::gate::Metric;
use crate::lang::{
    MeasuredFunction, NativeAnalysis, attached_overrides, directive_metrics, named_children, tree_sitter_error,
};

/// A file's syntax, for lining up revisions; empty when it does not parse.
pub fn syntax(source: &str) -> crate::lang::Syntax {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python grammar");
    parser.parse(source, None).map_or_else(Default::default, |tree| {
        crate::lang::tree_syntax(tree.root_node(), &["string", "integer", "float"])
    })
}

pub fn analyse_python(source: &str) -> NativeAnalysis {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python grammar");
    let Some(tree) = parser.parse(source, None) else {
        return NativeAnalysis {
            error: Some("could not parse".into()),
            ..Default::default()
        };
    };
    if let Some(error) = tree_sitter_error(tree.root_node()) {
        return NativeAnalysis {
            error: Some(error),
            ..Default::default()
        };
    }
    let file = File {
        source,
        comments: crate::lang::comments(tree.root_node(), source),
    };
    let mut out = NativeAnalysis::default();
    let mut names = crate::lang::Names::default();
    let mut defs = Vec::new();
    collect_defs(tree.root_node(), None, &file, &mut defs);
    let graph = CallGraph::of(tree.root_node(), &file);
    for (def, decorated, class) in defs {
        let own = file.text(def.child_by_field_name("name").expect("def name"));
        // `@total.setter def total(…)` is the property's setter: `Order.total.setter`.
        let accessor = decorated.and_then(|d| property_accessor(d, own, &file));
        let qualified = match accessor {
            Some(kind) => format!("{own}.{kind}"),
            None => own.to_string(),
        };
        let base = class.as_ref().map_or(qualified.clone(), |c| format!("{c}.{qualified}"));
        let name = names.add(base, None);
        let line = def.start_position().row as u32 + 1;
        let recursive = graph.recursion(def);
        out.functions.push(MeasuredFunction {
            origin: names.origin(&name),
            name: name.clone(),
            line,
            cyclomatic: 1 + radon_decisions(body(def), &file),
            cognitive: root_cognitive(def, &file) + recursive,
            span: def.byte_range(),
        });
        let first_line = decorated.unwrap_or(def).start_position().row as u32 + 1;
        out.overrides.extend(attached_overrides(
            &file.comments,
            line,
            first_line,
            &name,
            comment_metrics,
        ));
    }
    out.findings = crate::slop::python(tree.root_node(), source, &file.comments);
    out.overrides.extend(crate::slop::tree_lint_overrides(&file.comments));
    out
}

struct File<'s> {
    source: &'s str,
    /// The comments the parser found: only these can be overrides.
    comments: std::collections::HashMap<u32, crate::lang::Comment<'s>>,
}

impl File<'_> {
    fn text(&self, node: Node) -> &str {
        &self.source[node.byte_range()]
    }
}

/// `# noqa: C901` covers cyclomatic; `# noqa: complexipy` and `# complexipy: ignore` cognitive.
fn comment_metrics(comment: &str) -> Vec<Metric> {
    let mut metrics = directive_metrics(comment);
    let lower = comment.to_lowercase();
    if let Some(codes) = lower
        .split("noqa")
        .nth(1)
        .and_then(|rest| rest.trim_start().strip_prefix(':'))
    {
        let codes: Vec<&str> = codes
            .split([',', ' ', '\t', '#'])
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect();
        if codes.contains(&"c901") && !metrics.contains(&Metric::Cyclomatic) {
            metrics.push(Metric::Cyclomatic);
        }
        if codes.iter().any(|c| c.starts_with("complexipy")) && !metrics.contains(&Metric::Cognitive) {
            metrics.push(Metric::Cognitive);
        }
    }
    if lower.contains("complexipy: ignore") && !metrics.contains(&Metric::Cognitive) {
        metrics.push(Metric::Cognitive);
    }
    metrics
}

fn body(def: Node) -> Node {
    def.child_by_field_name("body").expect("def body")
}

/// The definition inside a `decorated_definition`, or the node itself.
fn definition(node: Node) -> Node {
    if node.kind() == "decorated_definition" {
        node.child_by_field_name("definition").unwrap_or(node)
    } else {
        node
    }
}

type Def<'t> = (Node<'t>, Option<Node<'t>>, Option<String>);

/// Top-level functions and class methods, as complexipy collects them: through classes and through
/// compound statements, but not into function bodies.
fn collect_defs<'t>(node: Node<'t>, class: Option<&str>, file: &File, out: &mut Vec<Def<'t>>) {
    for child in named_children(node) {
        let def = definition(child);
        let decorated = (def != child).then_some(child);
        match def.kind() {
            "function_definition" if !decorated.is_some_and(|d| is_overload(d, file)) => {
                out.push((def, decorated, class.map(str::to_string)));
            }
            "function_definition" => {}
            "class_definition" => {
                let own = file.text(def.child_by_field_name("name").expect("class name"));
                let qualified = class.map_or(own.to_string(), |c| format!("{c}.{own}"));
                collect_defs(body(def), Some(&qualified), file, out);
            }
            _ => collect_defs(child, class, file, out),
        }
    }
}

/// `setter` or `deleter` for a function decorated `@name.setter` or `@name.deleter`.
fn property_accessor(decorated: Node, name: &str, file: &File) -> Option<&'static str> {
    named_children(decorated)
        .into_iter()
        .filter(|c| c.kind() == "decorator")
        .find_map(
            |d| match file.text(d).trim_start_matches('@').trim().strip_prefix(name)? {
                ".setter" => Some("setter"),
                ".deleter" => Some("deleter"),
                _ => None,
            },
        )
}

/// `@overload` (or `@typing.overload`): a signature for type checkers, with no body to measure.
fn is_overload(decorated: Node, file: &File) -> bool {
    named_children(decorated)
        .into_iter()
        .filter(|c| c.kind() == "decorator")
        .any(|d| {
            let text = file.text(d).trim_start_matches('@').trim();
            text == "overload" || text.ends_with(".overload")
        })
}

/// radon's decision points in a function body, nested functions included.
fn radon_decisions(node: Node, file: &File) -> u32 {
    let own = match node.kind() {
        "if_statement" | "elif_clause" | "conditional_expression" | "assert_statement" => 1,
        "boolean_operator" => 1,
        "for_statement" | "while_statement" => 1 + u32::from(node.child_by_field_name("alternative").is_some()),
        "try_statement" => named_children(node)
            .iter()
            .filter(|c| matches!(c.kind(), "except_clause" | "except_group_clause" | "else_clause"))
            .count() as u32,
        "for_in_clause" | "if_clause" if is_comprehension_part(node) => 1,
        "match_statement" => match_cases(node, file),
        _ => 0,
    };
    // Decorators and default values are outside radon's count, nested definitions included; radon
    // counts an `assert` without looking inside it, and leaves out a class defined in a function.
    let children: Vec<Node> = match node.kind() {
        "assert_statement" | "class_definition" => Vec::new(),
        "decorated_definition" => node.child_by_field_name("definition").into_iter().collect(),
        "function_definition" => vec![body(node)],
        _ => named_children(node),
    };
    own + children.into_iter().map(|c| radon_decisions(c, file)).sum::<u32>()
}

fn is_comprehension_part(node: Node) -> bool {
    node.parent().is_some_and(|p| {
        matches!(
            p.kind(),
            "list_comprehension" | "set_comprehension" | "dictionary_comprehension" | "generator_expression"
        )
    })
}

/// A `match` counts its cases, less one when a case catches everything (`case _:` or `case x:`).
fn match_cases(node: Node, file: &File) -> u32 {
    let cases: Vec<Node> = node
        .child_by_field_name("body")
        .map(named_children)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.kind() == "case_clause")
        .collect();
    let catch_all = cases.iter().any(|case| {
        let patterns: Vec<Node> = named_children(*case)
            .into_iter()
            .filter(|c| c.kind() == "case_pattern")
            .collect();
        patterns.len() == 1 && {
            let text = file.text(patterns[0]).trim();
            text == "_"
                || (text.chars().all(|c| c.is_alphanumeric() || c == '_') && !text.contains('.'))
                    && !text.chars().next().is_some_and(|c| c.is_ascii_digit())
                    && !matches!(text, "None" | "True" | "False")
        }
    });
    (cases.len() as u32).saturating_sub(u32::from(catch_all))
}

/// The complexipy score of a function itself: its body from nesting 0, or for a decorator
/// factory, its wrapper's body from nesting 0.
fn root_cognitive(def: Node, file: &File) -> u32 {
    match decorator_inner(def, file) {
        Some((inner, returned)) => {
            header(inner, 0, file) + root_cognitive(definition(inner), file) + statement(returned, 0, file)
        }
        None => suite(body(def), 0, file),
    }
}

/// A decorator factory: a body that is only (after a docstring) a nested `def` and a `return` of
/// it, or of a call that takes it as a direct argument, such as `functools.wraps(f)(inner)`.
fn decorator_inner<'t>(def: Node<'t>, file: &File) -> Option<(Node<'t>, Node<'t>)> {
    if def.kind() != "function_definition" {
        return None;
    }
    let mut statements = named_children(body(def));
    statements.retain(|s| s.kind() != "comment");
    if statements
        .first()
        .is_some_and(|s| s.kind() == "expression_statement" && is_string(*s))
    {
        statements.remove(0);
    }
    let [inner, returned] = statements.as_slice() else {
        return None;
    };
    let inner_def = definition(*inner);
    if inner_def.kind() != "function_definition" || returned.kind() != "return_statement" {
        return None;
    }
    let inner_name = file.text(inner_def.child_by_field_name("name")?);
    let is_inner = |n: Node| n.kind() == "identifier" && file.text(n) == inner_name;
    let value = named_children(*returned).into_iter().next()?;
    let returns_inner = if value.kind() == "call" {
        let function = value.child_by_field_name("function")?;
        let args = value
            .child_by_field_name("arguments")
            .map(named_children)
            .unwrap_or_default();
        !is_inner(function)
            && args.iter().any(|a| {
                is_inner(*a) || (a.kind() == "keyword_argument" && a.child_by_field_name("value").is_some_and(is_inner))
            })
    } else {
        is_inner(value)
    };
    returns_inner.then_some((*inner, *returned))
}

fn is_string(statement: Node) -> bool {
    named_children(statement)
        .first()
        .is_some_and(|e| matches!(e.kind(), "string" | "concatenated_string"))
}

/// Decorators and default values of a definition, scored as complexipy scores a definition header.
fn header(node: Node, nesting: u32, file: &File) -> u32 {
    let mut score = 0;
    if node.kind() == "decorated_definition" {
        for decorator in named_children(node).into_iter().filter(|c| c.kind() == "decorator") {
            score += named_children(decorator)
                .into_iter()
                .map(|e| bool_ops(e, nesting, file))
                .sum::<u32>();
        }
    }
    let def = definition(node);
    match def.kind() {
        "function_definition" => {
            for parameter in def
                .child_by_field_name("parameters")
                .map(named_children)
                .unwrap_or_default()
            {
                if let Some(default) = parameter.child_by_field_name("value") {
                    score += bool_ops(default, nesting, file);
                }
            }
        }
        "class_definition" => {
            for argument in def
                .child_by_field_name("superclasses")
                .map(named_children)
                .unwrap_or_default()
            {
                let value = argument.child_by_field_name("value").unwrap_or(argument);
                score += bool_ops(value, nesting, file);
            }
        }
        _ => {}
    }
    score
}

fn suite(block: Node, nesting: u32, file: &File) -> u32 {
    named_children(block)
        .into_iter()
        .map(|s| header(s, nesting, file) + statement(s, nesting, file))
        .sum()
}

fn exprs(node: Node, nesting: u32, file: &File) -> u32 {
    named_children(node)
        .into_iter()
        .map(|e| bool_ops(e, nesting, file))
        .sum()
}

/// complexipy's score for one statement at a nesting level.
fn statement(node: Node, nesting: u32, file: &File) -> u32 {
    if let Some((inner, returned)) = decorator_inner(definition(node), file) {
        return header(inner, nesting + 1, file)
            + statement(inner, nesting, file)
            + statement(returned, nesting + 1, file);
    }
    let field = |name: &str| node.child_by_field_name(name);
    let block = |name: &str, n: u32| field(name).map_or(0, |b| suite(b, n, file));
    match node.kind() {
        "decorated_definition" => statement(definition(node), nesting, file),
        "function_definition" => block("body", nesting + 1),
        "class_definition" => block("body", nesting),
        "if_statement" => if_statement(node, nesting, file),
        "for_statement" | "while_statement" => {
            let controls: u32 = ["left", "right", "condition"]
                .iter()
                .filter_map(|f| field(f))
                .map(|e| bool_ops(e, nesting, file))
                .sum();
            let mut score = 1 + nesting + controls + block("body", nesting + 1);
            if let Some(orelse) = field("alternative") {
                score += 1 + orelse
                    .child_by_field_name("body")
                    .map_or(0, |b| suite(b, nesting + 1, file));
            }
            score
        }
        "try_statement" => try_statement(node, nesting, file),
        "with_statement" => {
            let items: u32 = named_children(node)
                .into_iter()
                .filter(|c| c.kind() == "with_clause")
                .flat_map(named_children)
                .map(|item| exprs(item, nesting, file))
                .sum();
            items + block("body", nesting)
        }
        "match_statement" => {
            let mut cursor = node.walk();
            let subjects: u32 = node
                .children_by_field_name("subject", &mut cursor)
                .map(|s| bool_ops(s, nesting, file))
                .sum();
            let mut score = 1 + nesting + subjects;
            for case in field("body")
                .map(named_children)
                .unwrap_or_default()
                .into_iter()
                .filter(|c| c.kind() == "case_clause")
            {
                if let Some(guard) = case.child_by_field_name("guard") {
                    score += exprs(guard, nesting, file);
                }
                score += case
                    .child_by_field_name("consequence")
                    .map_or(0, |b| suite(b, nesting + 1, file));
            }
            score
        }
        "expression_statement" | "return_statement" | "raise_statement" | "assert_statement" | "delete_statement" => {
            exprs(node, nesting, file)
        }
        _ => 0,
    }
}

/// `if` costs 1 plus nesting; each `elif` and `else` costs 1; their bodies sit one level deeper.
fn if_statement(node: Node, nesting: u32, file: &File) -> u32 {
    let condition = node
        .child_by_field_name("condition")
        .map_or(0, |c| bool_ops(c, nesting, file));
    let consequence = node
        .child_by_field_name("consequence")
        .map_or(0, |b| suite(b, nesting + 1, file));
    let mut score = 1 + nesting + condition + consequence;
    let mut cursor = node.walk();
    for clause in node.children_by_field_name("alternative", &mut cursor) {
        let condition = clause
            .child_by_field_name("condition")
            .map_or(0, |c| bool_ops(c, nesting, file));
        let body = clause
            .child_by_field_name("consequence")
            .or_else(|| clause.child_by_field_name("body"));
        score += 1 + condition + body.map_or(0, |b| suite(b, nesting + 1, file));
    }
    score
}

/// Each `except` costs 1 plus nesting, a `try`'s `else` costs 1, and `finally` costs nothing.
fn try_statement(node: Node, nesting: u32, file: &File) -> u32 {
    let mut score = node.child_by_field_name("body").map_or(0, |b| suite(b, nesting, file));
    for clause in named_children(node) {
        let block = named_children(clause).into_iter().find(|c| c.kind() == "block");
        score += match clause.kind() {
            "except_clause" | "except_group_clause" => {
                let types: u32 = named_children(clause)
                    .into_iter()
                    .filter(|c| c.kind() != "block")
                    .map(|t| bool_ops(t, nesting, file))
                    .sum();
                1 + nesting + types + block.map_or(0, |b| suite(b, nesting + 1, file))
            }
            "else_clause" => {
                1 + clause
                    .child_by_field_name("body")
                    .map_or(0, |b| suite(b, nesting + 1, file))
            }
            "finally_clause" => block.map_or(0, |b| suite(b, nesting, file)),
            _ => 0,
        };
    }
    score
}

fn unparen(node: Node) -> Node {
    if node.kind() == "parenthesized_expression"
        && let Some(inner) = named_children(node).into_iter().next()
    {
        return unparen(inner);
    }
    node
}

/// complexipy's score for an expression: runs of boolean operators, conditional expressions and
/// lambdas, wherever they sit inside it.
fn bool_ops(node: Node, nesting: u32, file: &File) -> u32 {
    match node.kind() {
        "boolean_operator" => {
            let mut operators = Vec::new();
            let mut operands = Vec::new();
            flatten(node, file, &mut operators, &mut operands);
            let runs = operators.windows(2).filter(|w| w[0] != w[1]).count() as u32 + 1;
            runs + operands.into_iter().map(|o| bool_ops(o, nesting, file)).sum::<u32>()
        }
        "conditional_expression" => {
            let parts = named_children(node);
            let [body, test, orelse] = parts.as_slice() else {
                return 0;
            };
            1 + nesting
                + bool_ops(*test, nesting, file)
                + bool_ops(*body, nesting + 1, file)
                + bool_ops(*orelse, nesting + 1, file)
        }
        "lambda" => {
            let defaults: u32 = node
                .child_by_field_name("parameters")
                .map(named_children)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.child_by_field_name("value"))
                .map(|d| bool_ops(d, nesting, file))
                .sum();
            defaults
                + node
                    .child_by_field_name("body")
                    .map_or(0, |b| bool_ops(b, nesting + 1, file))
        }
        _ => named_children(node)
            .into_iter()
            .map(|c| bool_ops(c, nesting, file))
            .sum(),
    }
}

/// The operators of a boolean chain in source order, through parentheses, and its other operands.
fn flatten<'t>(node: Node<'t>, file: &File, operators: &mut Vec<String>, operands: &mut Vec<Node<'t>>) {
    let node = unparen(node);
    if node.kind() != "boolean_operator" {
        operands.push(node);
        return;
    }
    if let Some(left) = node.child_by_field_name("left") {
        flatten(left, file, operators, operands);
    }
    if let Some(op) = node.child_by_field_name("operator") {
        operators.push(file.text(op).to_string());
    }
    if let Some(right) = node.child_by_field_name("right") {
        flatten(right, file, operators, operands);
    }
}

/// Every function in the module (top-level, methods and nested), with what it calls, for
/// complexipy's recursion: a function on a call cycle costs 1.
struct CallGraph {
    defs: Vec<GraphDef>,
    /// Each `def`'s recursion count, by its tree-sitter id: see `recursion`.
    recursion: HashMap<usize, u32>,
}

/// One function in the module's call graph.
struct GraphDef {
    /// The tree-sitter id of its `def`.
    node_id: usize,
    name: String,
    /// The class whose instance `self` and `cls` refer to, if any (a method, or a function nested in one).
    class: Option<usize>,
    /// Whether it is defined directly in a class body.
    is_method: bool,
    /// The function it is nested in.
    parent: Option<usize>,
    /// Functions called by bare name.
    functions: HashSet<String>,
    /// Methods called through `self` or `cls`.
    methods: HashSet<String>,
    /// Names bound in its own scope (parameters, assignments, imports, nested defs), which shadow outer ones.
    bound: HashSet<String>,
}

impl CallGraph {
    fn of(root: Node, file: &File) -> CallGraph {
        let mut graph = CallGraph {
            defs: Vec::new(),
            recursion: HashMap::new(),
        };
        graph.walk(root, None, false, None, file);
        graph.recursion = graph.count_recursion();
        graph
    }

    /// 1 for each function on a call cycle, summed into every function it is nested in.
    fn count_recursion(&self) -> HashMap<usize, u32> {
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, def) in self.defs.iter().enumerate() {
            by_name.entry(&def.name).or_default().push(index);
        }
        let edges: Vec<_> = (0..self.defs.len()).map(|from| self.edges(from, &by_name)).collect();
        let mut counts: Vec<u32> = on_cycle(&edges).into_iter().map(u32::from).collect();
        // `walk` pushes a def before the defs nested in it, so a reverse pass sums inner first.
        for index in (0..self.defs.len()).rev() {
            if let Some(parent) = self.defs[index].parent {
                counts[parent] += counts[index];
            }
        }
        self.defs.iter().map(|d| d.node_id).zip(counts).collect()
    }

    fn walk(&mut self, node: Node, class: Option<usize>, in_class_body: bool, parent: Option<usize>, file: &File) {
        for child in named_children(node) {
            match child.kind() {
                "function_definition" => {
                    let (functions, methods) = calls(body(child), file);
                    let bound = bindings(child, file);
                    let name = child
                        .child_by_field_name("name")
                        .map_or("", |n| file.text(n))
                        .to_string();
                    let index = self.defs.len();
                    self.defs.push(GraphDef {
                        node_id: child.id(),
                        name,
                        class,
                        is_method: in_class_body,
                        parent,
                        functions,
                        methods,
                        bound,
                    });
                    self.walk(body(child), class, false, Some(index), file);
                }
                "class_definition" => self.walk(body(child), Some(child.id()), true, parent, file),
                "block" => self.walk(child, class, in_class_body, parent, file),
                _ => self.walk(child, class, false, parent, file),
            }
        }
    }

    /// Whether a bare-name call from `caller` resolves to `def`: the nearest scope binding the name wins.
    fn in_scope(&self, def: usize, caller: usize) -> bool {
        let target = self.defs[def].parent;
        let mut scope = Some(caller);
        while let Some(s) = scope {
            if target == Some(s) {
                return true;
            }
            if self.defs[s].bound.contains(&self.defs[def].name) {
                return false;
            }
            scope = self.defs[s].parent;
        }
        target.is_none() && !self.defs[def].is_method && self.defs[def].class.is_none()
    }

    fn edges(&self, from: usize, by_name: &HashMap<&str, Vec<usize>>) -> Vec<usize> {
        let caller = &self.defs[from];
        let named = caller.functions.iter().chain(&caller.methods);
        named
            .filter_map(|name| by_name.get(name.as_str()))
            .flatten()
            .copied()
            .filter(|&to| {
                let target = &self.defs[to];
                if target.is_method {
                    caller.class.is_some() && target.class == caller.class && caller.methods.contains(&target.name)
                } else {
                    caller.functions.contains(&target.name) && self.in_scope(to, from)
                }
            })
            .collect()
    }

    /// 1 for the function if it is on a call cycle, and 1 for each function nested in it that is.
    fn recursion(&self, def: Node) -> u32 {
        self.recursion.get(&def.id()).copied().unwrap_or(0)
    }
}

/// Which nodes lie on a cycle: those in a strongly connected component of more than one node,
/// or with an edge to themselves.
fn on_cycle(edges: &[Vec<usize>]) -> Vec<bool> {
    let mut tarjan = Tarjan {
        edges,
        order: vec![UNSEEN; edges.len()],
        low: vec![0; edges.len()],
        open: vec![false; edges.len()],
        stack: Vec::new(),
        out: vec![false; edges.len()],
        next: 0,
    };
    for root in 0..edges.len() {
        if tarjan.order[root] == UNSEEN {
            tarjan.search(root);
        }
    }
    tarjan.out
}

const UNSEEN: usize = usize::MAX;

/// Tarjan's strongly connected components, iterative so a deep call chain cannot overflow the
/// stack.
struct Tarjan<'a> {
    edges: &'a [Vec<usize>],
    /// The order each node was first reached in.
    order: Vec<usize>,
    /// The earliest node reachable from each, through nodes still open.
    low: Vec<usize>,
    /// Whether each node is on `stack`, its component not yet closed.
    open: Vec<bool>,
    stack: Vec<usize>,
    out: Vec<bool>,
    next: usize,
}

impl Tarjan<'_> {
    fn search(&mut self, root: usize) {
        self.enter(root);
        // Each frame is a node and how many of its edges have been followed.
        let mut frames = vec![(root, 0)];
        while let Some(frame) = frames.last_mut() {
            let node = frame.0;
            let Some(&to) = self.edges[node].get(frame.1) else {
                frames.pop();
                self.leave(node, frames.last().map(|f| f.0));
                continue;
            };
            frame.1 += 1;
            if self.order[to] == UNSEEN {
                self.enter(to);
                frames.push((to, 0));
            } else if self.open[to] {
                self.low[node] = self.low[node].min(self.order[to]);
            }
        }
    }

    fn enter(&mut self, node: usize) {
        (self.order[node], self.low[node]) = (self.next, self.next);
        self.next += 1;
        self.stack.push(node);
        self.open[node] = true;
    }

    /// Done with `node`: pass its reach to the node it was reached from, and close its
    /// component if it is the component's first node.
    fn leave(&mut self, node: usize, from: Option<usize>) {
        if let Some(parent) = from {
            self.low[parent] = self.low[parent].min(self.low[node]);
        }
        if self.low[node] != self.order[node] {
            return;
        }
        let mut component = Vec::new();
        while let Some(member) = self.stack.pop() {
            self.open[member] = false;
            component.push(member);
            if member == node {
                break;
            }
        }
        let cycles = component.len() > 1 || self.edges[node].contains(&node);
        for member in component {
            self.out[member] = cycles;
        }
    }
}

/// The names a function binds in its own scope. Comprehensions and lambdas have their own scopes.
/// Like complexipy, a `global` declaration does not undo the shadowing.
fn bindings(def: Node, file: &File) -> HashSet<String> {
    let mut bound = HashSet::new();
    parameters(def, file, &mut bound);
    let mut stack = named_children(body(def));
    while let Some(node) = stack.pop() {
        if bind(node, file, &mut bound) {
            stack.extend(named_children(node));
        }
    }
    bound
}

/// The parameter names of a function or lambda.
fn parameters(def: Node, file: &File, bound: &mut HashSet<String>) {
    if let Some(params) = def.child_by_field_name("parameters") {
        for param in named_children(params) {
            let name = param.child_by_field_name("name").or_else(|| param.named_child(0));
            targets(name.unwrap_or(param), file, bound);
        }
    }
}

/// Records the names `node` binds. False when its children need no visit: another scope, or done.
/// A comprehension is visited: its loop variables are its own, but a walrus in it binds out here.
fn bind(node: Node, file: &File, bound: &mut HashSet<String>) -> bool {
    let target = match node.kind() {
        "function_definition" | "class_definition" => {
            if let Some(name) = node.child_by_field_name("name") {
                targets(name, file, bound);
            }
            return false;
        }
        "import_statement" | "import_from_statement" => {
            imports(node, file, bound);
            return false;
        }
        "lambda" => return false,
        "assignment" | "augmented_assignment" | "for_statement" => node.child_by_field_name("left"),
        "named_expression" => node.child_by_field_name("name"),
        // `with … as name` and `case … as name`: the name comes last.
        "as_pattern" => node.named_child(node.named_child_count().saturating_sub(1) as u32),
        // `case [*rest]` and `case {**rest}`.
        "splat_pattern" => Some(node),
        // A one-part name in a case pattern is a capture; `Color.RED` is a value.
        "dotted_name" if capture(node) => Some(node),
        _ => None,
    };
    if let Some(target) = target {
        targets(target, file, bound);
    }
    true
}

fn capture(node: Node) -> bool {
    node.named_child_count() == 1
        && node
            .parent()
            .is_some_and(|p| matches!(p.kind(), "case_pattern" | "keyword_pattern"))
}

/// `import a.b` binds `a`; `import a as b` and `from m import a as b` bind `b`.
fn imports(node: Node, file: &File, bound: &mut HashSet<String>) {
    let module = node.child_by_field_name("module_name").map(|m| m.id());
    for name in named_children(node).into_iter().filter(|n| Some(n.id()) != module) {
        let name = name.child_by_field_name("alias").unwrap_or(name);
        let first = if name.kind() == "dotted_name" {
            name.named_child(0).unwrap_or(name)
        } else {
            name
        };
        bound.insert(file.text(first).to_string());
    }
}

/// The plain names an assignment-like target binds (not attributes or subscripts).
fn targets(node: Node, file: &File, bound: &mut HashSet<String>) {
    match node.kind() {
        "identifier" => {
            bound.insert(file.text(node).to_string());
        }
        "attribute" | "subscript" => {}
        _ => named_children(node)
            .into_iter()
            .for_each(|child| targets(child, file, bound)),
    }
}

#[derive(Default)]
struct Calls {
    functions: HashSet<String>,
    methods: HashSet<String>,
}

/// The calls written in a function body, outside nested functions: bare names (`name(…)`), and
/// names called through `self` or `cls`. A bare name a lambda or comprehension binds is not counted.
fn calls(node: Node, file: &File) -> (HashSet<String>, HashSet<String>) {
    let mut found = Calls::default();
    found.visit(node, file, &HashSet::new());
    (found.functions, found.methods)
}

impl Calls {
    fn visit(&mut self, node: Node, file: &File, outer: &HashSet<String>) {
        let inner;
        let hidden = match node.kind() {
            "function_definition" | "class_definition" => return,
            "lambda"
            | "list_comprehension"
            | "set_comprehension"
            | "dictionary_comprehension"
            | "generator_expression" => {
                inner = scope_names(node, file, outer);
                &inner
            }
            "call" => {
                self.record(node, file, outer);
                outer
            }
            _ => outer,
        };
        let enclosing = enclosing_part(node);
        for child in named_children(node) {
            let names = if Some(child.id()) == enclosing { outer } else { hidden };
            self.visit(child, file, names);
        }
    }

    fn record(&mut self, call: Node, file: &File, hidden: &HashSet<String>) {
        let Some(function) = call.child_by_field_name("function") else {
            return;
        };
        match function.kind() {
            "identifier" if !hidden.contains(file.text(function)) => {
                self.functions.insert(file.text(function).to_string());
            }
            "attribute" => {
                let object = function.child_by_field_name("object").map(|o| file.text(o));
                if let Some(object @ ("self" | "cls")) = object
                    && !hidden.contains(object)
                    && let Some(attribute) = function.child_by_field_name("attribute")
                {
                    self.methods.insert(file.text(attribute).to_string());
                }
            }
            _ => {}
        }
    }
}

/// The part of a lambda or comprehension that runs in the enclosing scope: a lambda's parameters
/// (their defaults) and a comprehension's first `for` clause (its iterable).
fn enclosing_part(node: Node) -> Option<usize> {
    let part = match node.kind() {
        "lambda" => node.child_by_field_name("parameters"),
        "list_comprehension" | "set_comprehension" | "dictionary_comprehension" | "generator_expression" => {
            named_children(node).into_iter().find(|c| c.kind() == "for_in_clause")
        }
        _ => None,
    };
    part.map(|p| p.id())
}

/// The names hidden inside a lambda (its parameters) or a comprehension (its loop variables).
fn scope_names(node: Node, file: &File, outer: &HashSet<String>) -> HashSet<String> {
    let mut names = outer.clone();
    parameters(node, file, &mut names);
    for clause in named_children(node).into_iter().filter(|c| c.kind() == "for_in_clause") {
        if let Some(left) = clause.child_by_field_name("left") {
            targets(left, file, &mut names);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(source: &str) -> Vec<(String, u32, u32)> {
        let a = analyse_python(source);
        assert!(a.error.is_none(), "{:?}", a.error);
        a.functions
            .into_iter()
            .map(|f| (f.name, f.cyclomatic, f.cognitive))
            .collect()
    }

    #[test]
    fn matches_radon_and_complexipy() {
        // radon 12, complexipy 10 for this function.
        let src = "def f(a, b, c, d):\n    if a and (b or c) and d:\n        return [x for x in a if x] if b else None\n    try:\n        pass\n    except ValueError:\n        pass\n    else:\n        pass\n    while a:\n        pass\n    else:\n        pass\n";
        assert_eq!(scores(src), [("f".into(), 12, 10)]);
    }

    #[test]
    fn names() {
        let src = "from typing import overload\nclass A:\n    @property\n    def v(self): return 1\n    @v.setter\n    def v(self, x): pass\n    class B:\n        def m(self): pass\nif True:\n    def g(): pass\n@overload\ndef h(x: int) -> int: ...\ndef h(x): return x\ndef outer():\n    def inner(): pass\n";
        let names: Vec<String> = scores(src).into_iter().map(|(n, ..)| n).collect();
        assert_eq!(names, ["A.v", "A.v.setter", "A.B.m", "g", "h", "outer"]);
    }

    #[test]
    fn class_inside_a_function() {
        // radon 6.0.1: `outer` 2 and no entry for `Inner.m`. complexipy 9.0.0: `outer` 4.
        let src = "def outer(x):\n    class Inner:\n        def m(self, y):\n            if y and x:\n                return 1\n            return 2\n    if x:\n        return Inner\n";
        assert_eq!(scores(src), [("outer".into(), 2, 4)]);
    }

    #[test]
    fn local_bindings_shadow_the_function() {
        // complexipy 9.0.0 scores: a name bound in the function's own scope is not a call to itself.
        let src = "def f(f, x):\n    if x:\n        return f()\ndef g(x):\n    g = len\n    if x:\n        return g()\ndef h(x, *h):\n    if x:\n        return h()\ndef k(x: int, **k):\n    if x:\n        return k()\ndef m(x):\n    for m in x:\n        if x:\n            m()\ndef p(x):\n    import p\n    if x:\n        return p()\ndef q(x):\n    if x:\n        q()\n    q = 1\ndef t(x):\n    with x as t:\n        if x:\n            t()\ndef v(x):\n    if (v := x):\n        v()\ndef o(o=1):\n    def inner():\n        if o:\n            o()\ndef n(x):\n    if x:\n        return n()\ndef r(x):\n    global r\n    r = 1\n    if x:\n        r()\ndef s(x, y=lambda s: s()):\n    if x:\n        return s()\ndef u(x):\n    [u for u in x]\n    if x:\n        u()\n";
        // Match captures, comprehension and lambda scopes (complexipy 9.0.0).
        let src2 = "def m1(x):\n    match x:\n        case m1:\n            return m1()\ndef m2(x):\n    match x:\n        case [m2, *_]:\n            return m2()\ndef m3(x):\n    match x:\n        case {\"k\": 1, **m3}:\n            return m3()\ndef m4(x):\n    match x:\n        case int() as m4:\n            return m4()\ndef m5(x):\n    match x:\n        case Foo(a=m5):\n            return m5()\ndef m6(x):\n    match x:\n        case [*m6]:\n            return m6()\ndef m7(x):\n    match x:\n        case 1:\n            return m7()\ndef m8(x):\n    match x:\n        case m8.A:\n            return m8()\ndef c1(x):\n    return [c1() for c1 in x]\ndef c2(x):\n    return (lambda c2: c2())(x)\ndef c3(x):\n    return [c3() for y in x]\ndef c4(x):\n    return (lambda: c4())()\ndef c5(x):\n    return {k: c5() for k, c5 in x}\ndef c6(x):\n    return [y for c6 in x for y in c6()]\ndef c7(x):\n    return [c7() for y in x if (c7 := y)]\ndef c8(x):\n    return (lambda *c8: c8())(x)\n";
        let got2: Vec<(String, u32)> = scores(src2).into_iter().map(|(n, _, c)| (n, c)).collect();
        let want2 = [
            ("m1", 1),
            ("m2", 1),
            ("m3", 1),
            ("m4", 1),
            ("m5", 1),
            ("m6", 1),
            ("m7", 2),
            ("m8", 2),
            ("c1", 0),
            ("c2", 0),
            ("c3", 1),
            ("c4", 1),
            ("c5", 0),
            ("c6", 0),
            ("c7", 0),
            ("c8", 0),
        ];
        assert_eq!(got2, want2.map(|(n, c)| (n.to_string(), c)));
        // Lambda defaults and the first iterable run outside; a rebound receiver is not the method's.
        let src3 = "def d1(x):\n    if x:\n        return [d1 for d1 in d1(x)]\ndef d2(x):\n    if x:\n        return (lambda d2=d2(x): d2)()\ndef d3(x):\n    if x:\n        return [y for d3 in x for y in d3(x)]\ndef d5(x):\n    if x:\n        return [y for y in x if d5(y)]\nclass C:\n    def f(self, x):\n        if x:\n            return (lambda self: self.f())(x)\n    def g(self, x):\n        if x:\n            return [self.g() for self in x]\n    def h(self, x):\n        if x:\n            return (lambda: self.h())()\n    @classmethod\n    def k(cls, x):\n        if x:\n            return (lambda cls: cls.k())(x)\n";
        let got3: Vec<(String, u32)> = scores(src3).into_iter().map(|(n, _, c)| (n, c)).collect();
        let want3 = [
            ("d1", 2),
            ("d2", 2),
            ("d3", 1),
            ("d5", 2),
            ("C.f", 1),
            ("C.g", 1),
            ("C.h", 2),
            ("C.k", 1),
        ];
        assert_eq!(got3, want3.map(|(n, c)| (n.to_string(), c)));
        let got: Vec<(String, u32)> = scores(src).into_iter().map(|(n, _, c)| (n, c)).collect();
        let want = [
            ("f", 1),
            ("g", 1),
            ("h", 1),
            ("k", 1),
            ("m", 3),
            ("p", 1),
            ("q", 1),
            ("t", 1),
            ("v", 1),
            ("o", 2),
            ("n", 2),
            ("r", 1),
            ("s", 2),
            ("u", 2),
        ];
        assert_eq!(got, want.map(|(n, c)| (n.to_string(), c)));
    }

    #[test]
    fn on_cycle_finds_components_and_self_calls() {
        // 0 → 1 → 2 → 0 is a cycle; 3 → 0 only reaches it; 4 calls itself; 5 → 6 is a chain.
        let edges = vec![vec![1], vec![2], vec![0], vec![0], vec![4], vec![6], vec![]];
        assert_eq!(super::on_cycle(&edges), [true, true, true, false, true, false, false]);
    }

    #[test]
    fn recursion_counts_once_per_function_on_a_cycle() {
        let src =
            "def walk(n):\n    return walk(n - 1)\nclass S:\n    def a(self): self.b()\n    def b(self): self.a()\n";
        assert_eq!(
            scores(src),
            [("walk".into(), 1, 1), ("S.a".into(), 1, 1), ("S.b".into(), 1, 1)]
        );
    }

    #[test]
    fn overrides() {
        let src = "def a():  # noqa: C901 -- reason\n    pass\n# complexipy: ignore\ndef b():\n    pass\ndef c():  # noqa: E501\n    pass\n# noqa: C901 -- one branch per code\n# complexipy: ignore\n@cache\ndef d():\n    pass\n";
        let found: Vec<_> = analyse_python(src)
            .overrides
            .into_iter()
            .map(|o| (o.function.unwrap(), o.metric))
            .collect();
        assert_eq!(
            found,
            [
                ("a".into(), Metric::Cyclomatic),
                ("b".into(), Metric::Cognitive),
                ("d".into(), Metric::Cyclomatic),
                ("d".into(), Metric::Cognitive)
            ]
        );
    }

    #[test]
    fn directives_inside_strings_are_not_overrides() {
        let src = r#"def f(a="quoted \" # noqa: C901 -- docs"):
    pass
s = '''
# complexipy: ignore
'''
def g():
    pass
"#;
        assert!(analyse_python(src).overrides.is_empty());
    }
}
