//! Anti-slop for Rust, Python and Go: ports of its ideas, on the trees the
//! language analysers already parse. Each rejects a pattern that claims more than the code has proved:
//! a value of any type crossing a function boundary, a dictionary of anything, a cast or `unsafe`
//! with no stated invariant, an assertion that panics instead of checking.
//!
//! Findings are reported as `anti-slop-rust(…)`, `anti-slop-python(…)` and `anti-slop-go(…)`.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use crate::gate::Metric;
use crate::lang::{Comment, NativeOverride, directive_rules, named_children};
use crate::python_cast::{Api, Typing, unparenthesized};

pub struct NativeFinding {
    /// Byte offset, to find the function the finding sits in.
    pub offset: usize,
    /// Where the reported code ends, by byte.
    pub end: usize,
    pub line: u32,
    /// As reported: `anti-slop-go(no-any-parameters)`.
    pub rule: String,
    pub message: &'static str,
}

type Rule = (&'static str, &'static str);

const RUST: [Rule; 4] = [
    (
        "require-safety-comment-for-unsafe",
        "This `unsafe` has no `// SAFETY:` justification. State the invariant that makes it sound on the line above.",
    ),
    (
        "no-transmute",
        "`transmute` fabricates a type without evidence. Convert with `from_*` constructors, `bytemuck` or a parser.",
    ),
    (
        "no-unwrap",
        "`unwrap()` asserts without evidence and panics with no context. Propagate with `?`, handle the case, or use `expect(\"<the invariant>\")`.",
    ),
    (
        "no-dyn-any",
        "`dyn Any` in a signature leaves the value's type unknown to callers. Take and return a named type or an enum of the cases.",
    ),
];

const PYTHON: [Rule; 5] = [
    (
        "no-any-parameters",
        "Parameter typed `Any` leaves input unparsed. Accept a named domain type; parse at the I/O boundary before calling this function.",
    ),
    (
        "no-any-returns",
        "Returning `Any` exposes an unknown value to callers. Parse it at its boundary and return a named domain type.",
    ),
    (
        "no-object-parameters",
        "Parameter typed `object` accepts anything and proves nothing. Accept a named type or a Protocol.",
    ),
    (
        "no-unsafe-dictionary-type",
        "This dictionary's `Any`/`object` value type gives callers no contract. Use a TypedDict, dataclass or a concrete value type.",
    ),
    (
        "require-safety-comment-for-cast",
        "This `cast` has no `# SAFETY:` justification. State the checked invariant on the line above.",
    ),
];

const GO: [Rule; 5] = [
    (
        "no-any-parameters",
        "Parameter typed `any` leaves input unparsed. Accept a named type, a generic constraint or a small interface.",
    ),
    (
        "no-any-returns",
        "Returning `any` exposes an unknown value to callers. Return a named type.",
    ),
    (
        "no-unsafe-dictionary-type",
        "This map's `any` value type gives callers no contract. Use a struct or a concrete value type.",
    ),
    (
        "no-unchecked-type-assertion",
        "A single-value type assertion panics when it is wrong. Use `v, ok := x.(T)` or a type switch.",
    ),
    (
        "require-safety-comment-for-unsafe",
        "This use of package `unsafe` has no `// SAFETY:` justification. State the invariant that makes it sound on the line above.",
    ),
];

/// Rule names for each language's plugin prefix.
pub fn rules() -> impl Iterator<Item = (&'static str, &'static str)> {
    let names = |prefix: &'static str, rules: &'static [Rule]| rules.iter().map(move |(name, _)| (prefix, *name));
    names("anti-slop-rust", &RUST)
        .chain(names("anti-slop-python", &PYTHON))
        .chain(names("anti-slop-go", &GO))
}

fn finding(prefix: &str, rules: &[Rule], rule: &str, span: std::ops::Range<usize>, line: u32) -> NativeFinding {
    let (name, message) = rules.iter().find(|(n, _)| *n == rule).expect("known rule");
    NativeFinding {
        offset: span.start,
        end: span.end,
        line,
        rule: format!("{prefix}({name})"),
        message,
    }
}

/// `SAFETY: <reason>` in a comment on the line, or in the comments directly above it. Callers
/// also try the first line of the statement around the construct, for one that wraps onto it.
fn justified(line: u32, comment: impl Fn(u32) -> Option<(String, bool)>) -> bool {
    let says = |text: &str| {
        text.split_once("SAFETY:")
            .is_some_and(|(_, why)| !why.trim().is_empty())
    };
    if comment(line).is_some_and(|(text, _)| says(&text)) {
        return true;
    }
    let mut above = line.saturating_sub(1);
    while let Some((text, own_line)) = comment(above).filter(|(_, own_line)| *own_line) {
        if own_line && says(&text) {
            return true;
        }
        above = above.saturating_sub(1);
    }
    false
}

/// `autoratchet-disable-next-line <rule> -- reason` comments that name a lint rule: they approve
/// the finding on the next line, wherever it is in the file.
pub fn lint_overrides<'a>(comments: impl Iterator<Item = (u32, &'a str)>) -> Vec<NativeOverride> {
    comments
        .filter(|(_, text)| directive_rules(text).iter().any(|r| crate::collect::is_lint_rule(r)))
        .map(|(line, text)| NativeOverride {
            line,
            text: text.to_string(),
            metric: Metric::Lint,
            function: None,
        })
        .collect()
}

/// Lint overrides among a tree-sitter file's comments: those on their own line.
pub fn tree_lint_overrides(comments: &HashMap<u32, Comment>) -> Vec<NativeOverride> {
    let mut own: Vec<(u32, &str)> = comments
        .values()
        .filter(|c| c.starts_line)
        .map(|c| (c.line, c.text))
        .collect();
    own.sort_unstable();
    lint_overrides(own.into_iter())
}

/// Comments by line for tree-sitter languages, as `justified` reads them.
fn tree_comment<'a>(comments: &'a HashMap<u32, Comment>) -> impl Fn(u32) -> Option<(String, bool)> + 'a {
    |line| comments.get(&line).map(|c| (c.text.to_string(), c.starts_line))
}

// ---------------------------------------------------------------------------------------------
// Rust

pub fn rust(file: &syn::File, source: &str, comment_lines: &std::collections::HashSet<u32>) -> Vec<NativeFinding> {
    let lines: Vec<&str> = source.lines().collect();
    let mut visitor = RustSlop {
        comment: Box::new(move |line| {
            comment_lines
                .contains(&line)
                .then(|| lines.get(line as usize - 1).map(|l| (l.trim().to_string(), true)))
                .flatten()
        }),
        in_test: 0,
        scopes: vec![MemImports::of_items(&file.items)],
        statements: Vec::new(),
        out: Vec::new(),
    };
    visitor.visit_file(file);
    visitor.out
}

struct RustSlop<'a> {
    comment: Box<dyn Fn(u32) -> Option<(String, bool)> + 'a>,
    /// Inside `#[test]` functions or `#[cfg(test)]` modules, where `unwrap()` is the idiom.
    in_test: u32,
    /// What each enclosing scope binds, innermost last. A module starts afresh: it does not see
    /// its parent's imports; a block sees everything around it.
    scopes: Vec<MemImports>,
    /// The first line of each enclosing statement, innermost last: a `SAFETY:` comment above a
    /// statement that wraps onto the `unsafe` covers it.
    statements: Vec<u32>,
    out: Vec<NativeFinding>,
}

impl RustSlop<'_> {
    fn report(&mut self, rule: &str, span: proc_macro2::Span) {
        let line = span.start().line as u32;
        self.out
            .push(finding("anti-slop-rust", &RUST, rule, span.byte_range(), line));
    }

    fn unsafe_here(&mut self, span: proc_macro2::Span) {
        let line = span.start().line as u32;
        let statement = self.statements.last().copied().unwrap_or(line);
        if !justified(line, &self.comment) && !justified(statement, &self.comment) {
            self.report("require-safety-comment-for-unsafe", span);
        }
    }

    fn is_test(&self, attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            let last = a.path().segments.last().map(|s| s.ident.to_string());
            last.as_deref() == Some("test")
                || (a.path().is_ident("cfg") && a.parse_args::<syn::Meta>().is_ok_and(|m| only_in_tests(&m)))
        })
    }

    /// `std::mem::transmute` or `core::mem::transmute`, spelled out or imported. A project's
    /// own `mod mem` or `fn transmute` is not the standard library's.
    fn is_mem(&self, path: &syn::Path) -> bool {
        let segments: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        match segments.as_slice() {
            [name] => self.resolve(name) == Some(Binding::Transmute),
            [module, name] => self.resolve(module) == Some(Binding::Mem) && is_transmute(name),
            [root, module, name] => is_std_or_core(root) && module == "mem" && is_transmute(name),
            _ => false,
        }
    }

    /// What `name` means where it is used: the innermost scope that binds it decides.
    fn resolve(&self, name: &str) -> Option<Binding> {
        let has = |names: &[String]| names.iter().any(|n| n == name);
        self.scopes.iter().rev().find_map(|s| {
            if has(&s.functions) {
                Some(Binding::Transmute)
            } else if has(&s.modules) {
                Some(Binding::Mem)
            } else {
                has(&s.other).then_some(Binding::Other)
            }
        })
    }

    fn scoped(&mut self, test: bool, visit: impl FnOnce(&mut Self)) {
        self.in_test += u32::from(test);
        visit(self);
        self.in_test -= u32::from(test);
    }
}

fn is_std_or_core(name: &str) -> bool {
    matches!(name, "std" | "core")
}

fn is_transmute(name: &str) -> bool {
    matches!(name, "transmute" | "transmute_copy")
}

#[derive(Clone, Copy, PartialEq)]
enum Binding {
    /// `std::mem::transmute` or `transmute_copy`, under whatever name it was imported as.
    Transmute,
    /// `std::mem` itself.
    Mem,
    /// Anything else: a local `fn transmute`, a `mod mem`, an import from elsewhere.
    Other,
}

/// The names one scope binds, as far as `transmute` is concerned.
#[derive(Default)]
struct MemImports {
    modules: Vec<String>,
    functions: Vec<String>,
    other: Vec<String>,
}

impl MemImports {
    fn of_items<'a>(items: impl IntoIterator<Item = &'a syn::Item>) -> Self {
        let mut scope = MemImports::default();
        for item in items {
            match item {
                syn::Item::Use(u) => scope.collect(&u.tree, &mut Vec::new()),
                syn::Item::Fn(f) => scope.other.push(f.sig.ident.to_string()),
                syn::Item::Mod(m) => scope.other.push(m.ident.to_string()),
                _ => {}
            }
        }
        scope
    }

    fn collect(&mut self, tree: &syn::UseTree, path: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(segment) => {
                path.push(segment.ident.to_string());
                self.collect(&segment.tree, path);
                path.pop();
            }
            syn::UseTree::Group(group) => {
                for branch in &group.items {
                    self.collect(branch, path);
                }
            }
            syn::UseTree::Name(name) => {
                let original = name.ident.to_string();
                let bound = if original == "self" { "mem" } else { &original };
                self.bind(path, &original, bound);
            }
            syn::UseTree::Rename(rename) => {
                self.bind(path, &rename.ident.to_string(), &rename.rename.to_string());
            }
            syn::UseTree::Glob(_) => {}
        }
    }

    fn bind(&mut self, path: &[String], original: &str, bound: &str) {
        match path {
            [root] if is_std_or_core(root) && original == "mem" => self.modules.push(bound.into()),
            [root, module] if is_std_or_core(root) && module == "mem" => {
                if original == "self" {
                    self.modules.push(bound.into());
                } else if is_transmute(original) {
                    self.functions.push(bound.into());
                } else {
                    self.other.push(bound.into());
                }
            }
            _ => self.other.push(bound.into()),
        }
    }
}

/// Whether a `cfg` predicate holds only in test builds: `test` and `all(test, …)` do;
/// `not(test)` and `any(test, unix)` also hold in production.
fn only_in_tests(meta: &syn::Meta) -> bool {
    let syn::Meta::List(list) = meta else {
        return meta.path().is_ident("test");
    };
    let parts = list.parse_args_with(syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated);
    let Ok(parts) = parts else {
        return false;
    };
    if list.path.is_ident("all") {
        parts.iter().any(only_in_tests)
    } else if list.path.is_ident("any") {
        !parts.is_empty() && parts.iter().all(only_in_tests)
    } else {
        false
    }
}

impl<'ast> Visit<'ast> for RustSlop<'_> {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        let items = m
            .content
            .as_ref()
            .map(|(_, items)| items.as_slice())
            .unwrap_or_default();
        let outer = std::mem::replace(&mut self.scopes, vec![MemImports::of_items(items)]);
        self.scoped(self.is_test(&m.attrs), |s| visit::visit_item_mod(s, m));
        self.scopes = outer;
    }

    fn visit_stmt(&mut self, statement: &'ast syn::Stmt) {
        self.statements.push(statement.span().start().line as u32);
        visit::visit_stmt(self, statement);
        self.statements.pop();
    }

    fn visit_block(&mut self, b: &'ast syn::Block) {
        let items = b.stmts.iter().filter_map(|s| match s {
            syn::Stmt::Item(item) => Some(item),
            _ => None,
        });
        self.scopes.push(MemImports::of_items(items));
        visit::visit_block(self, b);
        self.scopes.pop();
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.scoped(self.is_test(&f.attrs), |s| visit::visit_item_fn(s, f));
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.scoped(self.is_test(&f.attrs), |s| visit::visit_impl_item_fn(s, f));
    }

    fn visit_expr_unsafe(&mut self, e: &'ast syn::ExprUnsafe) {
        self.unsafe_here(e.unsafe_token.span);
        visit::visit_expr_unsafe(self, e);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if let Some(token) = &i.unsafety {
            self.unsafe_here(token.span);
        }
        visit::visit_item_impl(self, i);
    }

    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*c.func
            && let Some(last) = p.path.segments.last()
            && self.is_mem(&p.path)
        {
            self.report("no-transmute", last.ident.span());
        }
        visit::visit_expr_call(self, c);
    }

    fn visit_expr_method_call(&mut self, c: &'ast syn::ExprMethodCall) {
        // No exemption for `.lock().unwrap()`: without types, a mutex cannot be told from
        // `File::lock` or any other fallible `lock`. `expect("lock poisoned")` states it.
        if c.method == "unwrap" && c.args.is_empty() && self.in_test == 0 {
            self.report("no-unwrap", c.method.span());
        }
        visit::visit_expr_method_call(self, c);
    }

    fn visit_signature(&mut self, s: &'ast syn::Signature) {
        let inputs = s.inputs.iter().filter_map(|arg| match arg {
            syn::FnArg::Typed(t) => Some(&*t.ty),
            syn::FnArg::Receiver(_) => None,
        });
        let output = match &s.output {
            syn::ReturnType::Type(_, ty) => Some(&**ty),
            syn::ReturnType::Default => None,
        };
        let any: Vec<proc_macro2::Span> = inputs
            .chain(output)
            .filter(|t| has_dyn_any(t))
            .map(Spanned::span)
            .collect();
        for span in any {
            self.report("no-dyn-any", span);
        }
        visit::visit_signature(self, s);
    }
}

/// Integration tests, benches and test helpers (`tests/`, `benches/`, `usage_test_support.rs`):
/// test code, where `unwrap()` is the idiom, even outside `#[cfg(test)]`.
pub fn is_rust_test_file(path: &str) -> bool {
    let parts: Vec<&str> = path.split(['/', '\\']).collect();
    let stem = parts.last().map_or("", |f| f.trim_end_matches(".rs"));
    parts[..parts.len().saturating_sub(1)]
        .iter()
        .any(|d| matches!(*d, "tests" | "benches"))
        || stem.split('_').any(|w| matches!(w, "test" | "tests" | "testing"))
}

fn has_dyn_any(ty: &syn::Type) -> bool {
    struct Finder(bool);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_type_trait_object(&mut self, t: &'ast syn::TypeTraitObject) {
            let traits: Vec<String> = t
                .bounds
                .iter()
                .filter_map(|b| match b {
                    syn::TypeParamBound::Trait(t) => t.path.segments.last().map(|s| s.ident.to_string()),
                    _ => None,
                })
                .collect();
            // `dyn Any + Send` is a panic payload, the type `catch_unwind` and `panic::set_hook`
            // hand you; it is not a choice the code made.
            let payload = traits.len() == 2 && traits.iter().any(|t| t == "Send");
            self.0 |= !payload && traits.iter().any(|t| t == "Any");
            visit::visit_type_trait_object(self, t);
        }
    }
    let mut finder = Finder(false);
    finder.visit_type(ty);
    finder.0
}

// ---------------------------------------------------------------------------------------------
// Python and Go: one walk over every node.

pub(crate) fn walk<'t>(root: tree_sitter::Node<'t>, mut each: impl FnMut(tree_sitter::Node<'t>)) {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        each(node);
        stack.extend(node.named_children(&mut cursor).collect::<Vec<_>>());
    }
}

/// The statement or declaration `node` sits in: the ancestor whose parent is a block or the file.
fn statement_of(node: tree_sitter::Node) -> tree_sitter::Node {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if matches!(parent.kind(), "block" | "module" | "source_file" | "statement_list") {
            break;
        }
        current = parent;
    }
    current
}

fn line_of(node: tree_sitter::Node) -> u32 {
    node.start_position().row as u32 + 1
}

/// Split `a | b`, or the arguments of `X[a, b]`, at the top level only.
fn split_top(text: &str, separator: char) -> Vec<&str> {
    let (mut depth, mut start, mut parts) = (0i32, 0, Vec::new());
    for (i, c) in text.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            c if c == separator && depth == 0 => {
                parts.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts
}

/// Whether a Python annotation is `name`, or a union that admits it:
/// `Any`, `typing.Any`, `Any | None`, `Optional[Any]`, `Union[int, Any]`.
fn python_admits(annotation: &str, name: &str) -> bool {
    let t = annotation.trim().trim_matches(|c| c == '"' || c == '\'');
    let bare = t.rsplit('.').next().unwrap_or(t);
    if bare == name {
        return true;
    }
    let parts = split_top(t, '|');
    if parts.len() > 1 {
        return parts.iter().any(|p| python_admits(p, name));
    }
    let inner = |prefix: &str| {
        t.strip_prefix(prefix)
            .or_else(|| t.strip_prefix(&format!("typing.{prefix}")))
            .and_then(|rest| rest.strip_suffix(']'))
    };
    match (inner("Optional["), inner("Union[")) {
        (Some(arg), _) => python_admits(arg, name),
        (_, Some(args)) => split_top(args, ',').iter().any(|a| python_admits(a, name)),
        _ => false,
    }
}

static PY_DICT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:typing\.|collections\.abc\.)?(?:dict|Dict|Mapping|MutableMapping)\[(?s)(.*)\]$")
        .expect("valid regex")
});

pub fn python(root: tree_sitter::Node, source: &str, comments: &HashMap<u32, Comment>) -> Vec<NativeFinding> {
    let mut file = TreeFile::new("anti-slop-python", &PYTHON, source, comments);
    let mut typing = Typing::of(root, source);
    walk(root, |node| match node.kind() {
        "function_definition" => file.python_function(node),
        "subscript" | "generic_type" => file.python_dictionary(node),
        "call" => file.python_cast(node, &mut typing),
        _ => {}
    });
    file.out
}

/// One Python or Go file being checked, and what it found.
struct TreeFile<'a> {
    prefix: &'static str,
    rules: &'static [Rule],
    source: &'a str,
    comments: &'a HashMap<u32, Comment<'a>>,
    out: Vec<NativeFinding>,
}

impl<'a> TreeFile<'a> {
    fn new(
        prefix: &'static str,
        rules: &'static [Rule],
        source: &'a str,
        comments: &'a HashMap<u32, Comment<'a>>,
    ) -> Self {
        TreeFile {
            prefix,
            rules,
            source,
            comments,
            out: Vec::new(),
        }
    }

    fn text(&self, node: tree_sitter::Node) -> &'a str {
        &self.source[node.byte_range()]
    }

    fn report(&mut self, rule: &str, node: tree_sitter::Node) {
        let found = finding(self.prefix, self.rules, rule, node.byte_range(), line_of(node));
        self.out.push(found);
    }

    fn justified(&self, node: tree_sitter::Node) -> bool {
        let comment = tree_comment(self.comments);
        justified(line_of(node), &comment) || justified(line_of(statement_of(node)), &comment)
    }

    fn python_function(&mut self, node: tree_sitter::Node) {
        // Python's data model fixes a dunder method's signature: `__eq__(self, other: object)`,
        // `__getattr__(self, name) -> Any`.
        if is_dunder(node.child_by_field_name("name").map_or("", |n| self.text(n))) {
            return;
        }
        let returns = node.child_by_field_name("return_type");
        if let Some(r) = returns.filter(|r| python_admits(self.text(*r), "Any")) {
            self.report("no-any-returns", r);
        }
        // A type guard is where an unknown value gets checked: it may take one.
        let guard = returns.is_some_and(|r| {
            let head = self.text(r).split('[').next().unwrap_or("").trim();
            matches!(head.rsplit('.').next(), Some("TypeGuard" | "TypeIs"))
        });
        if guard {
            return;
        }
        let params = node
            .child_by_field_name("parameters")
            .map(named_children)
            .unwrap_or_default();
        for param in params {
            self.python_parameter(param);
        }
    }

    fn python_parameter(&mut self, param: tree_sitter::Node) {
        let Some(annotation) = python_annotation(param) else {
            return;
        };
        if python_admits(self.text(annotation), "Any") {
            self.report("no-any-parameters", annotation);
        } else if python_admits(self.text(annotation), "object") && !self.python_sentinel(param) {
            self.report("no-object-parameters", annotation);
        }
    }

    /// `default_: object = NO_DEFAULT`: `object` lets a module-level sentinel tell "not passed"
    /// apart from `None`. The sentinel's name is a constant: upper case.
    fn python_sentinel(&self, param: tree_sitter::Node) -> bool {
        param.child_by_field_name("value").is_some_and(|v| {
            let name = self.text(v);
            v.kind() == "identifier"
                && name.chars().any(|c| c.is_ascii_uppercase())
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        })
    }

    fn python_dictionary(&mut self, node: tree_sitter::Node) {
        let Some(caps) = PY_DICT_RE.captures(self.text(node)) else {
            return;
        };
        // The last argument is the value type; `dict[str, Any,]` ends in an empty one.
        let args = split_top(caps.get(1).map_or("", |m| m.as_str()), ',');
        let value = args.into_iter().rfind(|a| !a.is_empty()).unwrap_or("");
        if python_admits(value, "Any") || python_admits(value, "object") {
            self.report("no-unsafe-dictionary-type", node);
        }
    }

    fn python_cast(&mut self, node: tree_sitter::Node, typing: &mut Typing<'a>) {
        let callee = node
            .child_by_field_name("function")
            .map_or("", |f| self.text(unparenthesized(f)));
        if typing.calls(node, callee, Api::Cast) && !self.justified(node) {
            self.report("require-safety-comment-for-cast", node);
        }
    }

    fn go_parameter(&mut self, node: tree_sitter::Node) {
        // A named result, `(out any)`, is a return: go_result reports it.
        let list = node.parent();
        let owner = list.and_then(|l| l.parent());
        if owner.and_then(|o| o.child_by_field_name("result")) == list {
            return;
        }
        if node.child_by_field_name("type").is_some_and(|t| go_any(t, self.source)) {
            for _ in 0..go_names(node) {
                self.report("no-any-parameters", node);
            }
        }
    }

    fn go_result(&mut self, node: tree_sitter::Node) {
        let Some(result) = node.child_by_field_name("result") else {
            return;
        };
        // One position per name: `(a, b any)` returns two values, as `(a any, b any)` does.
        let types: Vec<_> = if result.kind() == "parameter_list" {
            named_children(result)
                .into_iter()
                .filter_map(|p| Some((p.child_by_field_name("type")?, go_names(p))))
                .collect()
        } else {
            vec![(result, 1)]
        };
        for (any, positions) in types.into_iter().filter(|(t, _)| go_any(*t, self.source)) {
            for _ in 0..positions {
                self.report("no-any-returns", any);
            }
        }
    }

    fn go_map(&mut self, node: tree_sitter::Node) {
        if node
            .child_by_field_name("value")
            .is_some_and(|v| go_any(v, self.source))
        {
            self.report("no-unsafe-dictionary-type", node);
        }
    }

    fn go_unsafe(&mut self, node: tree_sitter::Node, unsafe_names: &[&str]) {
        let package = node
            .child_by_field_name("operand")
            .or_else(|| node.child_by_field_name("package"));
        let imported = package.is_some_and(|p| {
            let name = self.text(p);
            unsafe_names.contains(&name) && !go_local(node, name, self.source)
        });
        if imported && !self.justified(node) {
            self.report("require-safety-comment-for-unsafe", node);
        }
    }

    fn go_dot_unsafe(&mut self, node: tree_sitter::Node) {
        let ident = match node.kind() {
            "call_expression" => node
                .child_by_field_name("function")
                .filter(|f| f.kind() == "identifier"),
            _ => Some(node),
        };
        let Some(ident) = ident else {
            return;
        };
        let name = self.text(ident);
        let unsafe_api = matches!(
            name,
            "Pointer" | "Sizeof" | "Alignof" | "Offsetof" | "Add" | "Slice" | "SliceData" | "String" | "StringData"
        );
        if unsafe_api && !go_local(ident, name, self.source) && !self.justified(node) {
            self.report("require-safety-comment-for-unsafe", node);
        }
    }
}

/// `__eq__`, `__getattr__`: a method whose signature Python's data model fixes.
fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

/// A typed parameter's annotation. `*args: Any` and `**kwargs: Any` forward arguments: they
/// claim nothing, so they have none to check.
fn python_annotation(param: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let splat = named_children(param)
        .iter()
        .any(|c| c.kind().ends_with("splat_pattern"));
    if splat || !param.kind().starts_with("typed") {
        return None;
    }
    param.child_by_field_name("type")
}

/// How many values a parameter declaration stands for: its names, or one when unnamed.
fn go_names(declaration: tree_sitter::Node) -> usize {
    let mut cursor = declaration.walk();
    declaration.children_by_field_name("name", &mut cursor).count().max(1)
}

/// The names a Go file's imports give the `unsafe` package: `unsafe`, an alias, or `.`. A name
/// not among them is some other value, such as a parameter called `unsafe`.
fn go_unsafe_imports<'a>(root: tree_sitter::Node, source: &'a str) -> Vec<&'a str> {
    let mut names = Vec::new();
    walk(root, |node| {
        let path = node.child_by_field_name("path").map(|p| &source[p.byte_range()]);
        if node.kind() == "import_spec" && matches!(path, Some("\"unsafe\"" | "`unsafe`")) {
            names.push(
                node.child_by_field_name("name")
                    .map_or("unsafe", |n| &source[n.byte_range()]),
            );
        }
    });
    names
}

/// Whether `name`, used at `node`, is a local binding that hides the package of that name: a
/// parameter, receiver or result, or a declaration earlier in an enclosing block or header.
fn go_local(node: tree_sitter::Node, name: &str, source: &str) -> bool {
    let mut child = node;
    // `switch u := x.(type)` binds `u` in each clause's body, not in its `case` types.
    let mut case_type = false;
    while let Some(parent) = child.parent() {
        case_type |= parent.kind() == "type_case" && child.kind() != "statement_list";
        // Parameters, receivers and results are in scope in the body: not in the signature, nor
        // over one another's types.
        let function = matches!(
            parent.kind(),
            "function_declaration" | "method_declaration" | "func_literal"
        );
        let signature =
            (function && parent.child_by_field_name("body") != Some(child)) || parent.kind() == "parameter_list";
        let earlier = named_children(parent).into_iter().take_while(|c| c.id() != child.id());
        let mut binding = earlier.filter(|c| match c.kind() {
            "parameter_list" | "parameter_declaration" => !signature,
            "expression_list" => !case_type,
            _ => true,
        });
        if binding.any(|c| go_binds(c, name, source)) {
            return true;
        }
        child = parent;
    }
    false
}

/// Whether a declaration, parameter list or statement header binds `name`.
fn go_binds(node: tree_sitter::Node, name: &str, source: &str) -> bool {
    let is_name = |n: tree_sitter::Node| &source[n.byte_range()] == name;
    let named = |field: &str| {
        let mut cursor = node.walk();
        node.children_by_field_name(field, &mut cursor).any(is_name)
    };
    match node.kind() {
        // `u := …`, `for u := range …`, `case u := <-ch`, `switch u := x.(type)`.
        "short_var_declaration" | "range_clause" | "receive_statement" => node
            .child_by_field_name("left")
            .is_some_and(|left| named_children(left).into_iter().any(is_name)),
        "expression_list" => {
            let alias = node.parent().and_then(|p| p.child_by_field_name("alias"));
            alias == Some(node) && named_children(node).into_iter().any(is_name)
        }
        "var_spec"
        | "const_spec"
        | "parameter_declaration"
        | "variadic_parameter_declaration"
        | "type_spec"
        | "type_alias" => named("name"),
        "var_declaration" | "const_declaration" | "var_spec_list" | "parameter_list" | "for_clause" => {
            named_children(node).into_iter().any(|c| go_binds(c, name, source))
        }
        _ => false,
    }
}

/// `any` or `interface{}`.
fn go_any(node: tree_sitter::Node, source: &str) -> bool {
    match node.kind() {
        "type_identifier" | "identifier" => &source[node.byte_range()] == "any",
        "interface_type" => named_children(node).is_empty(),
        _ => false,
    }
}

/// `v, ok := x.(T)`, `v, ok = x.(T)` and `var v, ok = x.(T)`: the assertion that checks.
/// The assertion must be the only value on the right: `a, b := x.(int), 2` checks nothing.
fn go_comma_ok(assertion: tree_sitter::Node) -> bool {
    let mut value = assertion;
    while let Some(p) = value.parent().filter(|p| p.kind() == "parenthesized_expression") {
        value = p;
    }
    let Some(list) = value
        .parent()
        .filter(|p| p.kind() == "expression_list" && p.named_child_count() == 1)
    else {
        return false;
    };
    let Some(statement) = list.parent() else {
        return false;
    };
    match statement.kind() {
        "assignment_statement" | "short_var_declaration" => statement
            .child_by_field_name("left")
            .is_some_and(|left| named_children(left).len() == 2),
        "var_spec" => {
            let mut cursor = statement.walk();
            statement.children_by_field_name("name", &mut cursor).count() == 2
        }
        _ => false,
    }
}

pub fn go(root: tree_sitter::Node, source: &str, comments: &HashMap<u32, Comment>) -> Vec<NativeFinding> {
    let mut file = TreeFile::new("anti-slop-go", &GO, source, comments);
    let unsafe_names = go_unsafe_imports(root, source);
    let dot_unsafe = unsafe_names.contains(&".");
    walk(root, |node| match node.kind() {
        // `args ...any` (a variadic_parameter_declaration) forwards arguments, as fmt-style
        // wrappers do: not a contract, so not flagged.
        "parameter_declaration" => file.go_parameter(node),
        "function_declaration" | "method_declaration" | "func_literal" | "function_type" | "method_elem" => {
            file.go_result(node)
        }
        "map_type" => file.go_map(node),
        "type_assertion_expression" if !go_comma_ok(node) => file.report("no-unchecked-type-assertion", node),
        "selector_expression" | "qualified_type" => file.go_unsafe(node, &unsafe_names),
        // `import . "unsafe"`: `Pointer(p)`, `Sizeof(x)`, with no package name in front.
        "type_identifier" | "call_expression" if dot_unsafe => file.go_dot_unsafe(node),
        _ => {}
    });
    file.out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_transmute_import_paths() {
        let result = crate::rust::analyse_rust(
            "mod tools { pub fn transmute(x: u32) -> u32 { x } }\n\
             use {std::mem, tools::transmute};\n\
             use core :: mem :: transmute_copy as copy_bits;\n\
             fn local(x: u32) -> u32 { transmute(x) }\n\
             fn real(x: u32) -> i32 { unsafe { copy_bits(&x) } }\n\
             fn grouped(x: u32) -> i32 { unsafe { mem::transmute(x) } }",
        );
        let lines: Vec<_> = result
            .findings
            .iter()
            .filter(|finding| finding.rule == "anti-slop-rust(no-transmute)")
            .map(|finding| finding.line)
            .collect();
        assert_eq!(lines, [5, 6]);
    }

    #[test]
    fn python_cast_through_brackets() {
        let result =
            crate::python::analyse_python("from typing import cast\n(cast)(int, value)\n((cast))(int, value)\n");
        let mut lines: Vec<_> = result.findings.iter().map(|f| f.line).collect();
        lines.sort_unstable();
        assert_eq!(lines, [2, 3]);
    }

    #[test]
    fn python_cast_import_and_statement_order() {
        let result = crate::python::analyse_python(concat!(
            "from typing import get_origin as cast\n",
            "origin = cast(list[int])\n",
            "from typing import cast\n",
            "value = cast(int, 1)\n",
            "cast = str\n",
            "text = cast(1)\n",
            "def convert(cast):\n",
            "    text = cast(1)\n",
            "    from typing import cast\n",
            "    return cast(str, text)\n",
        ));
        let mut lines: Vec<_> = result.findings.iter().map(|finding| finding.line).collect();
        lines.sort_unstable();
        assert_eq!(lines, [4, 10]);
    }

    #[test]
    fn rust_transmute_scopes() {
        let result = crate::rust::analyse_rust(
            "use std::mem::transmute;\n\
             fn shadowed(x: u32) -> u32 { fn transmute(x: u32) -> u32 { x } transmute(x) }\n\
             fn local_use(x: u32) -> i32 { use std::mem::transmute_copy; unsafe { transmute_copy(&x) } }\n\
             mod inner { fn fresh(x: u32) -> u32 { transmute(x) } fn transmute(x: u32) -> u32 { x } }\n\
             fn outer(x: u32) -> i32 { unsafe { transmute(x) } }",
        );
        let lines: Vec<_> = result
            .findings
            .iter()
            .filter(|finding| finding.rule == "anti-slop-rust(no-transmute)")
            .map(|finding| finding.line)
            .collect();
        assert_eq!(lines, [3, 5]);
    }

    #[test]
    fn safety_comment_above_a_wrapped_statement() {
        let rust = crate::rust::analyse_rust(
            "fn f(p: *const u8) -> u8 {\n\
             // SAFETY: the caller passes a valid pointer.\n\
             let value = Some(\n\
                 unsafe { *p },\n\
             );\n\
             value.unwrap_or(0)\n\
             }",
        );
        assert!(
            rust.findings.is_empty(),
            "rust: {:?}",
            rust.findings.iter().map(|f| &f.rule).collect::<Vec<_>>()
        );
        let python = crate::python::analyse_python(concat!(
            "from typing import cast\n",
            "# SAFETY: the parser returns a str here.\n",
            "value = str(\n",
            "    cast(str, raw),\n",
            ")\n",
        ));
        assert!(
            python.findings.is_empty(),
            "python: {:?}",
            python.findings.iter().map(|f| &f.rule).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rust_unwrap_after_lock_is_flagged_too() {
        // `remote.lock()` may return any `Result`: only types could prove a mutex.
        let result = crate::rust::analyse_rust(
            "fn f(m: &std::sync::Mutex<u8>, remote: &Remote) -> usize {\n\
             let guard = m.lock().unwrap();\n\
             let held = m.lock().expect(\"lock poisoned\");\n\
             remote.lock().unwrap().len() + *guard + *held\n\
             }",
        );
        let lines: Vec<_> = result.findings.iter().map(|finding| finding.line).collect();
        assert_eq!(lines, [2, 4]);
    }

    #[test]
    fn python_qualified_cast_resolves_its_module() {
        let result = crate::python::analyse_python(concat!(
            "import typing\n",
            "import typing_extensions as te\n",
            "from typing import cast as c\n",
            "a = typing.cast(int, 1)\n",
            "b = te.cast(int, 1)\n",
            "d = c(int, 1)\n",
            "def run(typing):\n",
            "    return typing.cast(1)\n",
            "e = money.cast(1)\n",
        ));
        let mut lines: Vec<_> = result.findings.iter().map(|finding| finding.line).collect();
        lines.sort_unstable();
        assert_eq!(lines, [4, 5, 6]);
    }

    #[test]
    fn python_cast_scopes_of_loops_handlers_lambdas_and_comprehensions() {
        let result = crate::python::analyse_python(concat!(
            "from typing import cast\n",
            "for cast in callbacks:\n    cast(1)\n",
            "try:\n    pass\nexcept E as cast:\n    cast(1)\n",
            "from typing import cast\n",
            "f = lambda cast: cast(1)\n",
            "g = [cast(1) for cast in callbacks]\n",
            "h = lambda x: cast(x)\n",
        ));
        let lines: Vec<_> = result.findings.iter().map(|finding| finding.line).collect();
        assert_eq!(lines, [11]);
    }

    #[test]
    fn python_cast_in_class_bodies_and_after_global() {
        let result = crate::python::analyse_python(concat!(
            "from typing import cast\n",
            "class Shadow:\n    cast = str\n    x = cast(123)\n",
            "class Later:\n    x = cast(int, 1)\n    cast = str\n",
            "class Method:\n    cast = str\n    def m(self):\n        return cast(int, 1)\n",
            "def g():\n    global cast\n    from typing import cast\n    return cast(int, 1)\n",
        ));
        let mut lines: Vec<_> = result.findings.iter().map(|finding| finding.line).collect();
        lines.sort_unstable();
        assert_eq!(lines, [6, 11, 15]);
    }

    #[test]
    fn python_cast_through_branches_and_nested_globals() {
        let result = crate::python::analyse_python(concat!(
            "from typing import cast\n",
            "if flag:\n    cast = str\nelse:\n    cast(int, 1)\n",
            "def outer():\n    cast = str\n    def inner():\n        global cast\n        return cast(int, 1)\n",
            "def same():\n    if flag:\n        cast = str\n        return cast(1)\n",
        ));
        let mut lines: Vec<_> = result.findings.iter().map(|finding| finding.line).collect();
        lines.sort_unstable();
        assert_eq!(lines, [5, 10]);
    }

    #[test]
    fn python_cast_along_the_paths_that_reach_the_call() {
        let flagged = |source: &str| -> Vec<u32> {
            let result = crate::python::analyse_python(source);
            let mut lines: Vec<_> = result.findings.iter().map(|finding| finding.line).collect();
            lines.sort_unstable();
            lines
        };
        // Only the else branch reaches the call; there `cast` is `str`.
        assert!(flagged("if flag:\n    from typing import cast\nelse:\n    cast = str\n    cast(1)\n").is_empty());
        // Both branches replace typing's `cast` before the join.
        assert!(
            flagged("from typing import cast\nif flag:\n    cast = str\nelse:\n    cast = repr\ncast(1)\n").is_empty()
        );
        // One branch keeps it; elif and a missing else keep it.
        let kept = "from typing import cast\nif a:\n    cast = str\nelif b:\n    pass\ncast(int, 1)\n";
        assert_eq!(flagged(kept), [6]);
        // The first pass of a loop sees typing's `cast`; a later pass sees `str`.
        assert_eq!(
            flagged("from typing import cast\nfor x in xs:\n    cast(int, x)\n    cast = str\n"),
            [3]
        );
        // A handler may run before the body's rebinding did.
        let handled = "from typing import cast\ntry:\n    f()\n    cast = str\nexcept E:\n    pass\ncast(int, 1)\n";
        assert_eq!(flagged(handled), [7]);
        // A path that returns never reaches the call.
        let returned = "from typing import cast\ndef f(a):\n    global cast\n    if a:\n        return\n    cast = str\n    return cast(1)\n";
        assert!(flagged(returned).is_empty());
        // A `break` carries typing's `cast` past the rebinding; a `continue` skips it.
        let broke =
            "for x in xs:\n    from typing import cast\n    if x:\n        break\n    cast = str\ncast(int, 1)\n";
        assert_eq!(flagged(broke), [6]);
        let skipped =
            "from typing import cast\nfor x in xs:\n    cast(int, x)\n    if x:\n        continue\n    cast = str\n";
        assert_eq!(flagged(skipped), [3]);
        // Only a loop that ends without `break` runs its `else`.
        let unbroken = concat!(
            "cast = str\nfor x in xs:\n    if x:\n        from typing import cast\n        break\n",
            "else:\n    cast(1)\n",
        );
        assert!(flagged(unbroken).is_empty());
        // Nothing after an unconditional `break` runs.
        let dead = "cast = str\nfor x in xs:\n    break\n    from typing import cast\ncast(1)\n";
        assert!(flagged(dead).is_empty());
        // A `case` pattern captures the name for its body only.
        let captured = concat!(
            "from typing import cast\nmatch m:\n    case {\"handler\": cast}:\n        cast(1)\n",
            "    case Point(x=cast) | [*cast]:\n        cast(1)\n    case Color.cast:\n        cast(int, 1)\n",
        );
        assert_eq!(flagged(captured), [8]);
        // `finally` runs on the way out of a `break`, both ways.
        let imported = concat!(
            "cast = str\nfor x in xs:\n    try:\n        break\n    finally:\n",
            "        from typing import cast\ncast(int, 1)\n",
        );
        assert_eq!(flagged(imported), [7]);
        let replaced = concat!(
            "from typing import cast\nwhile True:\n    try:\n        break\n    finally:\n",
            "        cast = str\ncast(1)\n",
        );
        assert!(flagged(replaced).is_empty());
        // A `return` pending in `try` reaches the loop's exit through a `finally` that breaks.
        let redirected = concat!(
            "cast = str\ndef f(xs):\n    global cast\n    for x in xs:\n        try:\n            return\n",
            "        finally:\n            from typing import cast\n            break\n    return cast(int, 1)\n",
        );
        assert_eq!(flagged(redirected), [10]);
        // A guard's walrus binds before the case body.
        let guard = "from typing import cast\nmatch m:\n    case 1 if (cast := str):\n        cast(123)\n";
        assert!(flagged(guard).is_empty());
        // Deep nesting of `finally` and loops stays fast: each statement is summarised once
        // per entry state.
        // Inside a suppressing `with` too.
        let nest = |head: &str, mut indent: usize| {
            let mut deep =
                format!("from typing import cast\nfrom contextlib import suppress\ndef f():\n    global cast\n{head}");
            for _ in 0..40 {
                let (a, b, c) = (" ".repeat(indent), " ".repeat(indent + 4), " ".repeat(indent + 8));
                deep.push_str(&format!("{a}while x:\n{b}try:\n{c}pass\n{b}finally:\n"));
                indent += 8;
            }
            deep + &format!("{}cast(int, 1)\n", " ".repeat(indent))
        };
        for (deep, line) in [
            (nest("", 4), 4 + 4 * 40 + 1),
            (nest("    with suppress(ValueError):\n", 8), 5 + 4 * 40 + 1),
        ] {
            assert!(crate::python::analyse_python(&deep).error.is_none());
            let started = std::time::Instant::now();
            assert_eq!(flagged(&deep), [line]);
            assert!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "{:?}",
                started.elapsed()
            );
        }
        // A literal pattern is not a catch-all.
        let literal = "from typing import cast\nmatch m:\n    case True:\n        cast = str\ncast(int, 1)\n";
        assert_eq!(flagged(literal), [5]);
        let number = "from typing import cast\nmatch m:\n    case 1:\n        cast = str\ncast(int, 1)\n";
        assert_eq!(flagged(number), [5]);
        let capture = "from typing import cast\nmatch m:\n    case 1:\n        cast = str\n    case other:\n        cast = int\ncast(1)\n";
        assert!(flagged(capture).is_empty());
        // A last `case _:` leaves no path where no case ran; a guarded one does.
        let every = "from typing import cast\nmatch m:\n    case 1:\n        cast = str\n    case _:\n        cast = int\ncast(123)\n";
        assert!(flagged(every).is_empty());
        let guarded = "from typing import cast\nmatch m:\n    case 1:\n        cast = str\n    case _ if m:\n        cast = int\ncast(int, 1)\n";
        assert_eq!(flagged(guarded), [7]);
        // A `for` may run no times; `while True:` leaves only by `break`.
        let maybe = "from typing import cast\nfor x in xs:\n    cast = str\n    break\ncast(int, 1)\n";
        assert_eq!(flagged(maybe), [5]);
        let always = "from typing import cast\nwhile True:\n    cast = str\n    break\ncast(1)\n";
        assert!(flagged(always).is_empty());
        // `with` may swallow an exception raised before the rebinding.
        let suppressed = "from typing import cast\nimport contextlib\nwith contextlib.suppress(ValueError):\n    raise ValueError\n    cast = str\ncast(int, 1)\n";
        assert_eq!(flagged(suppressed), [6]);
        let raises = "from typing import cast\nimport pytest\nwith pytest.raises(E) as e:\n    f()\n    cast = str\ncast(int, 1)\n";
        assert_eq!(flagged(raises), [6]);
        // The manager resolves through its binding, not its spelling.
        let renamed = "from typing import cast\nfrom contextlib import suppress as ignore\nwith ignore(ValueError):\n    raise ValueError\n    cast = str\ncast(int, 1)\n";
        assert_eq!(flagged(renamed), [6]);
        let local = "from typing import cast\ndef suppress(*e):\n    return nullcontext()\nwith suppress(ValueError):\n    cast = str\ncast(1)\n";
        assert!(flagged(local).is_empty());
        let unittest = "import unittest\nclass T(unittest.TestCase):\n    def t(self):\n        from typing import cast\n        with self.assertRaises(E):\n            f()\n            cast = str\n        cast(int, 1)\n";
        assert_eq!(flagged(unittest), [8]);
        // Not a `TestCase`'s `assertRaises`; `suppress()` swallows nothing.
        let unrelated = unittest.replace("(unittest.TestCase)", "");
        assert!(flagged(&unrelated).is_empty());
        let nothing = "from typing import cast\nfrom contextlib import suppress\nwith suppress():\n    f()\n    cast = str\ncast(1)\n";
        assert!(flagged(nothing).is_empty());
        // `suppress(KeyError)` does not swallow `ValueError`, nor `NameError` from reading a name.
        let other = "from typing import cast\nfrom contextlib import suppress\nwith suppress(KeyError):\n    if True:\n        raise ValueError(\"no\")\n    cast = str\ncast(1)\n";
        assert!(flagged(other).is_empty());
        let base = other.replace("raise ValueError", "raise IndexError");
        assert_eq!(flagged(&base.replace("KeyError", "LookupError")), [7]);
        // What an inner `suppress` surely swallows does not reach the outer one.
        let inner = "from typing import cast\nfrom contextlib import suppress\nwith suppress(Exception):\n    with suppress(ValueError):\n        raise ValueError\n    cast = str\ncast(1)\n";
        assert!(flagged(inner).is_empty());
        let escapes = inner.replace("suppress(ValueError)", "suppress(KeyError)");
        assert_eq!(flagged(&escapes), [7]);
        // What a `try` inside surely handles does not reach the manager either.
        let handled = "from typing import cast\nfrom contextlib import suppress\nwith suppress(Exception):\n    try:\n        raise ValueError\n    except ValueError:\n        pass\n    cast = str\ncast(1)\n";
        assert!(flagged(handled).is_empty());
        let unhandled = handled.replace("except ValueError", "except KeyError");
        assert_eq!(flagged(&unhandled), [9]);
        let unknown = "from typing import cast\nfrom contextlib import suppress\nwith suppress(Exception):\n    with suppress(*errors):\n        raise ValueError\n    cast = str\ncast(1)\n";
        assert_eq!(flagged(unknown), [7]);
        // A raised `KeyError` skips `except ValueError:`; the first handler that surely catches
        // keeps it from later ones.
        let skipped = "from typing import cast\ndef f():\n    try:\n        raise KeyError\n    except ValueError:\n        from typing import cast\n    except KeyError:\n        cast = str\n    cast(1)\n";
        assert!(flagged(skipped).is_empty());
        let first = "from typing import cast\ndef f():\n    try:\n        raise KeyError\n    except LookupError:\n        cast = str\n    except KeyError:\n        from typing import cast\n    cast(1)\n";
        assert!(flagged(first).is_empty());
        let reached = skipped.replace("raise KeyError", "g()");
        assert_eq!(flagged(&reached), [9]);
        // `if a:` runs `a.__bool__()`, which may raise before the rebinding.
        let tested = "def f():\n    from typing import cast\n    try:\n        if a:\n            pass\n        cast = str\n    except ValueError:\n        pass\n    cast(int, 1)\n";
        assert_eq!(flagged(tested), [9]);
        // A literal test runs no code.
        assert!(flagged(&tested.replace("if a:", "if True:").replace("cast(int, 1)", "cast(1)")).is_empty());
        // `IOError` is `OSError`: the first handler catches it, the second is never reached.
        let alias = "from typing import cast\ndef f():\n    try:\n        raise OSError()\n    except IOError:\n        cast = str\n    except OSError:\n        from typing import cast\n    cast(1)\n";
        assert!(flagged(alias).is_empty());
        // Reading an unknown name may raise `NameError` before the assignment; a built-in may not.
        let unknown =
            "from typing import cast\ntry:\n    cast = optional_cast\nexcept NameError:\n    pass\ncast(int, 1)\n";
        assert_eq!(flagged(unknown), [6]);
        assert!(
            flagged(
                &unknown
                    .replace("optional_cast", "str")
                    .replace("cast(int, 1)", "cast(1)")
            )
            .is_empty()
        );
        // An unbound local raises `UnboundLocalError`, a `NameError`; so does a built-in name
        // the function assigns later.
        let unbound = "def f():\n    from typing import cast\n    try:\n        cast = x\n        x = 1\n    except UnboundLocalError:\n        pass\n    cast(int, 1)\n";
        assert_eq!(flagged(unbound), [8]);
        let local = "def f():\n    from typing import cast\n    try:\n        cast = str\n    except NameError:\n        pass\n    str = 1\n    cast(int, 1)\n";
        assert_eq!(flagged(local), [8]);
        assert!(flagged(&local.replace("    str = 1\n", "").replace("cast(int, 1)", "cast(1)")).is_empty());
        // A parameter, or a name assigned on every path, is bound: reading it raises nothing.
        let parameter = "def f(converter):\n    from typing import cast\n    try:\n        cast = converter\n    except NameError:\n        pass\n    cast(1)\n";
        assert!(flagged(parameter).is_empty());
        let assigned = "def f():\n    from typing import cast\n    str = int\n    try:\n        cast = str\n    except NameError:\n        pass\n    cast(1)\n";
        assert!(flagged(assigned).is_empty());
        let maybe = "def f():\n    from typing import cast\n    if a:\n        str = int\n    try:\n        cast = str\n    except NameError:\n        pass\n    cast(int, 1)\n";
        assert_eq!(flagged(maybe), [9]);
        // An exception leaving a `try` runs its `finally` before the manager swallows it.
        let finally = "from typing import cast\nfrom contextlib import suppress\nwith suppress(ValueError):\n    try:\n        raise ValueError\n    finally:\n        cast = str\ncast(1)\n";
        assert!(flagged(finally).is_empty());
        let empty = finally.replace("cast = str", "pass").replace("cast(1)", "cast(int, 1)");
        assert_eq!(flagged(&empty), [8]);
        // A function reads a name the module binds after the module has run.
        let module = "def helper(x):\n    return x\ndef f():\n    from typing import cast\n    try:\n        cast = helper\n    except NameError:\n        pass\n    cast(1)\n";
        assert!(flagged(module).is_empty());
        let missing = module
            .replace("cast = helper", "cast = nowhere")
            .replace("cast(1)", "cast(int, 1)");
        assert_eq!(flagged(&missing), [9]);
        let enclosing = "def g(helper):\n    def f():\n        from typing import cast\n        try:\n            cast = helper\n        except NameError:\n            pass\n        cast(1)\n";
        assert!(flagged(enclosing).is_empty());
        let class = "class C:\n    helper = 1\n    def f(self):\n        from typing import cast\n        try:\n            cast = helper\n        except NameError:\n            pass\n        cast(int, 1)\n";
        assert_eq!(flagged(class), [9]);
        let own = other.replace("raise ValueError(\"no\")", "raise Mine");
        assert_eq!(flagged(&own), [7]);
        // `nullcontext()` and other managers let exceptions through.
        assert!(flagged("from typing import cast\nwith nullcontext():\n    cast = str\ncast(1)\n").is_empty());
        // A walrus on the right of `and`/`or`, or in a branch of `x if c else y`, may not run.
        let skipped = "from typing import cast\nmatch m:\n    case _ if enabled and (cast := str):\n        pass\n    case _:\n        cast(int, 1)\n";
        assert_eq!(flagged(skipped), [6]);
        // Only the true outcome of `and` has run its right operand; only the false one of `or`.
        let passed =
            "from typing import cast\nmatch m:\n    case _ if enabled and (cast := str):\n        cast(value)\n";
        assert!(flagged(passed).is_empty());
        assert!(flagged("from typing import cast\nif a and (cast := str):\n    cast(1)\n").is_empty());
        let negated = "from typing import cast\nif not (a and (cast := str)):\n    pass\nelse:\n    cast(1)\n";
        assert!(flagged(negated).is_empty());
        assert_eq!(
            flagged("from typing import cast\nif a or (cast := str):\n    cast(int, 1)\n"),
            [3]
        );
        let or_else = "from typing import cast\nif a or (cast := str):\n    pass\nelse:\n    cast(1)\n";
        assert!(flagged(or_else).is_empty());
        let looped = "from typing import cast\nwhile a and (cast := str):\n    cast(1)\n";
        assert!(flagged(looped).is_empty());
        // Both alternatives of a ternary condition replace the binding.
        let ternary = "from typing import cast\nif ((cast := str) if enabled else (cast := int)):\n    cast(123)\n";
        assert!(flagged(ternary).is_empty());
        let one = "from typing import cast\nif ((cast := str) if enabled else other):\n    cast(int, 1)\n";
        assert_eq!(flagged(one), [3]);
        let assigned = "from typing import cast\nconverter = (cast := str) if enabled else (cast := int)\ncast(123)\n";
        assert!(flagged(assigned).is_empty());
        let outer = "from typing import cast\ncast = int if enabled else (cast := str)\ncast(123)\n";
        assert!(flagged(outer).is_empty());
        let half = "from typing import cast\nconverter = (cast := str) if enabled else other\ncast(int, 1)\n";
        assert_eq!(flagged(half), [3]);
        let either = "from typing import cast\nx = a if (cast := str) else b\ncast(1)\n";
        assert!(flagged(either).is_empty());
        let branch = "from typing import cast\nx = (cast := str) if a else b\ncast(int, 1)\n";
        assert_eq!(flagged(branch), [3]);
        // A failed guard has already run: the fallback sees its binding.
        let fallback = "from typing import cast\nmatch m:\n    case _ if (cast := str) and flag:\n        pass\n    case _:\n        cast(value)\n";
        assert!(flagged(fallback).is_empty());
        let refutable = "from typing import cast\nmatch m:\n    case 1 if (cast := str):\n        pass\n    case _:\n        cast(int, 1)\n";
        assert_eq!(flagged(refutable), [6]);
        // A function reads the module's name when called: after the rebinding too.
        let later = "from typing import cast\nif a:\n    cast = str\ndef f():\n    return cast(int, 1)\n";
        assert_eq!(flagged(later), [5]);
    }

    #[test]
    fn go_unsafe_follows_the_import() {
        let analyse = |source: &str| -> Vec<u32> {
            let result = crate::go::analyse_go(source);
            let mut lines: Vec<_> = result.findings.iter().map(|finding| finding.line).collect();
            lines.sort_unstable();
            lines
        };
        assert_eq!(
            analyse("package p\nimport u \"unsafe\"\nfunc f(x *int) u.Pointer {\n\treturn u.Pointer(x)\n}\n"),
            [3, 4]
        );
        assert!(analyse("package p\nfunc f(unsafe T) int {\n\treturn unsafe.Size\n}\n").is_empty());
        // A parameter, a `:=` and a range variable hide the import; outside them it is unsafe again.
        let shadowed = concat!(
            "package p\nimport u \"unsafe\"\n",
            "func f(u holder) int {\n\treturn u.Value\n}\n",
            "func g(h holder) int {\n\tu := h\n\treturn u.Value\n}\n",
            "func k(hs []holder) {\n\tfor _, u := range hs {\n\t\t_ = u.Value\n\t}\n\t_ = u.Sizeof(hs)\n}\n",
        );
        assert_eq!(analyse(shadowed), [14]);
        // A parameter is in scope in the body, not in the signature.
        let signature = "package p\nimport u \"unsafe\"\nfunc use(u int, p u.Pointer) int {\n\treturn u\n}\n";
        assert_eq!(analyse(signature), [3]);
        // A type switch's variable is in scope in the clause body, not in its case types.
        let switch = "package p\nimport u \"unsafe\"\nfunc f(x error) {\n\tswitch u := x.(type) {\n\tcase u.Pointer:\n\t\t_ = u\n\t}\n}\n";
        assert_eq!(analyse(switch), [5]);
        // A parameter hides a dot-imported name too.
        let dot =
            "package p\nimport . \"unsafe\"\nfunc f(Add func(int) int, x *int) {\n\t_ = Add(1)\n\t_ = Sizeof(x)\n}\n";
        assert_eq!(analyse(dot), [5]);
        assert_eq!(
            analyse(
                "package p\nimport . \"unsafe\"\nfunc f(x *int) uintptr {\n\t_ = Pointer(x)\n\treturn Sizeof(x)\n}\n"
            ),
            [4, 5]
        );
    }

    #[test]
    fn python_cast_bindings_in_every_form() {
        // Each function binds `cast` some other way before calling it: none is typing's.
        let result = crate::python::analyse_python(concat!(
            "from typing import cast\n",
            "def a(xs):\n    for cast in xs:\n        cast(1)\n",
            "def b(f):\n    with f as cast:\n        cast(1)\n",
            "def c():\n    try:\n        pass\n    except Exception as cast:\n        cast(1)\n",
            "def d(f):\n    if (cast := f):\n        cast(1)\n",
            "def e(p):\n    x, cast = p\n    cast(1)\n",
            "def f(ok):\n    if ok:\n        from other import cast\n    cast(1)\n",
        ));
        assert!(
            result.findings.is_empty(),
            "{:?}",
            result.findings.iter().map(|f| f.line).collect::<Vec<_>>()
        );

        // Typing's `cast`, reached through `global`, an `if` block or a `try` block.
        let result = crate::python::analyse_python(concat!(
            "cast = None\n",
            "def g():\n    global cast\n    from typing import cast\n",
            "def h():\n    return cast(int, 1)\n",
            "try:\n    from typing import cast as c\nexcept ImportError:\n    pass\n",
            "def i():\n    return c(int, 1)\n",
        ));
        let lines: Vec<_> = result.findings.iter().map(|f| f.line).collect();
        assert_eq!(
            lines,
            [12],
            "only the `c` call: `g`'s global import runs only when g() does"
        );
    }

    #[test]
    fn python_annotations() {
        for t in [
            "Any",
            "typing.Any",
            "Any | None",
            "Optional[Any]",
            "Union[int, Any]",
            "'Any'",
        ] {
            assert!(python_admits(t, "Any"), "{t}");
        }
        for t in ["int", "list[Any]", "dict[str, Any]", "AnyStr", "Callable[..., Any]"] {
            assert!(!python_admits(t, "Any"), "{t}");
        }
    }

    #[test]
    fn rust_test_files() {
        for p in [
            "tests/api.rs",
            "crates/x/benches/b.rs",
            "src/usage_test_support.rs",
            "src/foo_tests.rs",
        ] {
            assert!(is_rust_test_file(p), "{p}");
        }
        for p in ["src/latest.rs", "src/contest.rs", "src/main.rs"] {
            assert!(!is_rust_test_file(p), "{p}");
        }
    }

    #[test]
    fn safety_comments() {
        let comments = |line: u32| match line {
            3 => Some(("// SAFETY: the buffer outlives the pointer".to_string(), true)),
            4 => Some(("// and is never resized".to_string(), true)),
            7 => Some(("// SAFETY:".to_string(), true)),
            _ => None,
        };
        assert!(justified(5, comments));
        assert!(!justified(8, comments), "an empty reason is no reason");
        assert!(!justified(10, comments));
    }
}
