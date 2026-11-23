//! Whether a Python call is `typing.cast`, for `require-safety-comment-for-cast`: what the
//! callee's name holds where the call is, along the paths that reach it. Imports, assignments,
//! branches, loops, `try`, `with`, `match` and walruses bind it; exceptions go to the handlers
//! and suppressing managers that may catch them.

use std::collections::{HashMap, HashSet};

use crate::lang::named_children;
use crate::slop::walk;

/// An imported function a call may resolve to.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Api {
    /// `typing.cast`.
    Cast,
    /// `contextlib.suppress`, which swallows exceptions.
    Suppress,
    /// `pytest.raises`, which swallows the exception it expects.
    Raises,
}

impl Api {
    const ALL: [Api; 3] = [Api::Cast, Api::Suppress, Api::Raises];

    /// The modules that define it.
    fn modules(self) -> &'static [&'static str] {
        match self {
            Api::Cast => &["typing", "typing_extensions"],
            Api::Suppress => &["contextlib"],
            Api::Raises => &["pytest"],
        }
    }

    fn name(self) -> &'static str {
        match self {
            Api::Cast => "cast",
            Api::Suppress => "suppress",
            Api::Raises => "raises",
        }
    }
}

/// What a name used in a call must be bound to for the call to be the API.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Want {
    /// `from typing import cast` (or `cast as c`).
    Name(Api),
    /// `import typing` (or `typing as t`), for `typing.cast`.
    Module(Api),
}

/// What one file's `cast` calls resolve against, gathered once so each call costs a walk up
/// its scopes rather than a scan of the file.
pub(crate) struct Typing<'a> {
    source: &'a str,
    /// Names some import binds as each `Want` asks: a call through any other name is not it.
    imported: HashSet<(Want, &'a str)>,
    /// `with` statements, by id, whose manager swallows exceptions, and the exception types.
    suppressing: Swallows<'a>,
    /// Each scope's bindings of a name, scanned on first use.
    scans: HashMap<(usize, &'a str, Want), Bindings>,
}

impl<'a> Typing<'a> {
    pub(crate) fn of(root: tree_sitter::Node, source: &'a str) -> Self {
        let mut typing = Typing {
            source,
            imported: HashSet::new(),
            suppressing: HashMap::new(),
            scans: HashMap::new(),
        };
        let mut managers = vec![];
        walk(root, |node| {
            let mut cursor = node.walk();
            let names = node.children_by_field_name("name", &mut cursor);
            match node.kind() {
                "import_from_statement" => {
                    let module = node
                        .child_by_field_name("module_name")
                        .map_or("", |m| &source[m.byte_range()]);
                    for (bound, original) in names.map(|i| from_binding(i, source)) {
                        let apis = Api::ALL.into_iter().filter(|a| a.name() == original);
                        let apis = apis.filter(|a| a.modules().contains(&module));
                        typing.imported.extend(apis.map(|a| (Want::Name(a), bound)));
                    }
                }
                "import_statement" => {
                    for (bound, module) in names.map(|i| module_binding(i, source)) {
                        let apis = Api::ALL.into_iter().filter(|a| a.modules().contains(&module));
                        typing.imported.extend(apis.map(|a| (Want::Module(a), bound)));
                    }
                }
                "with_item" => managers.push(node),
                _ => {}
            }
        });
        for item in managers {
            if let Some((with, types)) = typing.swallowing(item) {
                typing.suppressing.entry(with).or_default().extend(types);
            }
        }
        typing
    }

    /// `callee(…)` at `node` calls the API: `cast(…)` or `c(…)` bound to typing's `cast`, or
    /// `typing.cast(…)` / `t.cast(…)` on a name bound to the typing module. Resolved where the
    /// call is, so a parameter or local of the same name is not the API.
    pub(crate) fn calls(&mut self, node: tree_sitter::Node, callee: &'a str, api: Api) -> bool {
        match callee.rsplit_once('.') {
            Some((module, name)) if name == api.name() && self.imported.contains(&(Want::Module(api), module)) => {
                self.resolve(node, module, Want::Module(api))
            }
            Some(_) => false,
            None if self.imported.contains(&(Want::Name(api), callee)) => self.resolve(node, callee, Want::Name(api)),
            None => false,
        }
    }

    /// The `with` statement of a manager that swallows exceptions, and the types it swallows:
    /// `suppress(…)` or `pytest.raises(…)` as imported, or `self.assertRaises(…)` in a
    /// `TestCase`. `suppress()` swallows nothing.
    fn swallowing(&mut self, item: tree_sitter::Node) -> Option<(usize, Vec<&'a str>)> {
        let value = item.child_by_field_name("value")?;
        let call = if value.kind() == "as_pattern" {
            value.named_child(0)?
        } else {
            value
        };
        let callee = &self.source[unparenthesized(call.child_by_field_name("function")?).byte_range()];
        let unittest =
            matches!(callee, "self.assertRaises" | "self.assertRaisesRegex") && in_test_case(call, self.source);
        let swallows = unittest || self.calls(call, callee, Api::Suppress) || self.calls(call, callee, Api::Raises);
        let types = exception_types(call, self.source);
        let with = item.parent()?.parent()?;
        (swallows && !types.is_empty()).then(|| (with.id(), types))
    }

    /// Whether `name`, used at `node`, is bound as `want` says: the nearest enclosing scope that
    /// binds it decides, by its last binding before the call. A parameter, a `def`, a class, an
    /// assignment, a `for`/`with`/`except` target, a walrus or an import of something else binds
    /// a different value. `global`/`nonlocal` defer to the enclosing scope.
    fn resolve(&mut self, node: tree_sitter::Node, name: &'a str, want: Want) -> bool {
        let lookup = Lookup {
            name,
            want,
            source: self.source,
            suppressing: &self.suppressing,
        };
        let call = node.start_byte();
        // Where the enclosing scope is read: at the call, or where the inner scope sits in it.
        let mut at = node;
        // Past a function or lambda, the scope is read whenever it is called: at `at` or later.
        let mut later = false;
        // A class body's names are seen by code directly in it, not by functions nested in it.
        let mut nested = false;
        // Past a function that declares the name `global`, only the module decides.
        let mut global = false;
        let mut scope = node.parent();
        while let Some(s) = scope {
            scope = s.parent();
            let Some(body) = scope_body(s).filter(|b| b.byte_range().contains(&call)) else {
                continue;
            };
            if (nested && s.kind() == "class_definition") || (global && s.kind() != "module") {
                continue;
            }
            let bindings = self
                .scans
                .entry((s.id(), name, want))
                .or_insert_with(|| lookup.scan(body));
            let reach = lookup.reach(s, body, bindings, at, later);
            if reach & TYPING != 0 {
                return true;
            }
            if reach & UNBOUND == 0 {
                return false;
            }
            nested = true;
            global = bindings.global;
            at = s;
            later |= matches!(s.kind(), "function_definition" | "lambda");
        }
        false
    }
}

/// `(cast)` is `cast`: brackets around a callee change nothing.
pub(crate) fn unparenthesized(mut node: tree_sitter::Node) -> tree_sitter::Node {
    while node.kind() == "parenthesized_expression" && node.named_child_count() == 1 {
        let Some(inner) = node.named_child(0) else { break };
        node = inner;
    }
    node
}

/// `with` statements by id, and the exception types their managers swallow.
type Swallows<'a> = HashMap<usize, Vec<&'a str>>;

/// Whether `node` sits in a method of a class whose base is a `…TestCase`.
// By the base's name: bases are not resolved across files.
fn in_test_case(node: tree_sitter::Node, source: &str) -> bool {
    let class = std::iter::successors(node.parent(), |n| n.parent()).find(|n| n.kind() == "class_definition");
    let bases = class
        .and_then(|c| c.child_by_field_name("superclasses"))
        .map(named_children);
    bases
        .unwrap_or_default()
        .into_iter()
        .any(|b| source[b.byte_range()].ends_with("TestCase"))
}

/// The exception types a manager's positional arguments name: `suppress(KeyError, OSError)`,
/// `raises((KeyError, OSError))`. Anything but a name could be any type: `UNKNOWN`.
fn exception_types<'a>(call: tree_sitter::Node, source: &'a str) -> Vec<&'a str> {
    let args = call
        .child_by_field_name("arguments")
        .map(named_children)
        .unwrap_or_default();
    let positional = args
        .into_iter()
        .filter(|a| !matches!(a.kind(), "keyword_argument" | "comment"));
    let named = |n: tree_sitter::Node| match n.kind() {
        "identifier" | "attribute" => &source[n.byte_range()],
        _ => UNKNOWN,
    };
    positional
        .flat_map(|a| {
            if a.kind() == "tuple" {
                named_children(a)
            } else {
                vec![a]
            }
        })
        .map(named)
        .collect()
}

/// An exception type that is not a name: it may be any type, or none.
const UNKNOWN: &str = "?";

/// The exception types a handler names; a bare `except:` catches everything.
fn handler_types<'a>(clause: tree_sitter::Node, source: &'a str) -> Vec<&'a str> {
    {
        let Some(value) = clause.child_by_field_name("value") else {
            return vec!["BaseException"];
        };
        let value = if value.kind() == "as_pattern" {
            value.named_child(0).unwrap_or(value)
        } else {
            value
        };
        let value = if value.kind() == "parenthesized_expression" {
            value.named_child(0).unwrap_or(value)
        } else {
            value
        };
        let types = if value.kind() == "tuple" {
            named_children(value)
        } else {
            vec![value]
        };
        let name = |t: tree_sitter::Node| match t.kind() {
            "identifier" | "attribute" => &source[t.byte_range()],
            _ => UNKNOWN,
        };
        types.into_iter().map(name).collect()
    }
}

/// Python's built-in exceptions and their bases.
const EXCEPTIONS: &[(&str, &str)] = &[
    ("Exception", "BaseException"),
    ("SystemExit", "BaseException"),
    ("KeyboardInterrupt", "BaseException"),
    ("GeneratorExit", "BaseException"),
    ("ArithmeticError", "Exception"),
    ("AssertionError", "Exception"),
    ("AttributeError", "Exception"),
    ("BufferError", "Exception"),
    ("EOFError", "Exception"),
    ("ImportError", "Exception"),
    ("LookupError", "Exception"),
    ("MemoryError", "Exception"),
    ("NameError", "Exception"),
    ("OSError", "Exception"),
    ("ReferenceError", "Exception"),
    ("RuntimeError", "Exception"),
    ("StopIteration", "Exception"),
    ("StopAsyncIteration", "Exception"),
    ("SyntaxError", "Exception"),
    ("SystemError", "Exception"),
    ("TypeError", "Exception"),
    ("ValueError", "Exception"),
    ("FloatingPointError", "ArithmeticError"),
    ("OverflowError", "ArithmeticError"),
    ("ZeroDivisionError", "ArithmeticError"),
    ("ModuleNotFoundError", "ImportError"),
    ("IndexError", "LookupError"),
    ("KeyError", "LookupError"),
    ("UnboundLocalError", "NameError"),
    ("BlockingIOError", "OSError"),
    ("ChildProcessError", "OSError"),
    ("ConnectionError", "OSError"),
    ("FileExistsError", "OSError"),
    ("FileNotFoundError", "OSError"),
    ("InterruptedError", "OSError"),
    ("IsADirectoryError", "OSError"),
    ("NotADirectoryError", "OSError"),
    ("PermissionError", "OSError"),
    ("ProcessLookupError", "OSError"),
    ("TimeoutError", "OSError"),
    ("BrokenPipeError", "ConnectionError"),
    ("ConnectionAbortedError", "ConnectionError"),
    ("ConnectionRefusedError", "ConnectionError"),
    ("ConnectionResetError", "ConnectionError"),
    ("NotImplementedError", "RuntimeError"),
    ("RecursionError", "RuntimeError"),
    ("IndentationError", "SyntaxError"),
    ("TabError", "IndentationError"),
    ("UnicodeError", "ValueError"),
    ("UnicodeDecodeError", "UnicodeError"),
    ("UnicodeEncodeError", "UnicodeError"),
    ("UnicodeTranslateError", "UnicodeError"),
];

/// A built-in exception's own name: `IOError` and `EnvironmentError` are `OSError`.
fn canonical(name: &str) -> &str {
    match name {
        "IOError" | "EnvironmentError" | "WindowsError" => "OSError",
        _ => name,
    }
}

/// Whether a handler for `caught` surely catches `raised`, or anything when `None`.
fn must_catch(caught: &str, raised: Option<&str>) -> bool {
    let (caught, raised) = (canonical(caught), raised.map(canonical));
    let base = |name: &str| EXCEPTIONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b);
    let known = raised.filter(|r| base(r).is_some());
    caught == "BaseException" || known.is_some_and(|r| std::iter::successors(Some(r), |n| base(n)).any(|n| n == caught))
}

/// Whether a handler for `caught` may catch a raised `raised`. Only built-ins are known: a
/// class of the file's own may derive from anything, and a name may hold any type.
// Assumes built-in names keep their meaning.
fn may_catch(caught: &str, raised: &str) -> bool {
    let (caught, raised) = (canonical(caught), canonical(raised));
    let known = |name: &str| name == "BaseException" || EXCEPTIONS.iter().any(|(n, _)| *n == name);
    if !known(caught) || !known(raised) {
        return true;
    }
    let base = |name: &str| EXCEPTIONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b);
    std::iter::successors(Some(raised), |n| base(n)).any(|n| n == caught)
}

/// The part of a scope its own bindings live in: a module, a function, lambda or class body, or
/// a comprehension.
fn scope_body(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    match node.kind() {
        "module" | "list_comprehension" | "set_comprehension" | "dictionary_comprehension" | "generator_expression" => {
            Some(node)
        }
        "function_definition" | "lambda" | "class_definition" => node.child_by_field_name("body"),
        _ => None,
    }
}

/// Where a binding takes effect: a loop or handler binds its target before its body runs.
fn bound_at(node: tree_sitter::Node) -> usize {
    match node.kind() {
        "for_statement" => node.child_by_field_name("body").map(|b| b.start_byte()),
        "except_clause" => node.child_by_field_name("alias").map(|a| a.end_byte()),
        "case_clause" => node.child_by_field_name("consequence").map(|b| b.start_byte()),
        _ => None,
    }
    .unwrap_or(node.end_byte())
}

/// The innermost operand holding `node` that may not run: the right of `and`/`or`, or a branch
/// of `x if c else y`.
fn skippable_operand(node: tree_sitter::Node) -> Option<std::ops::Range<usize>> {
    let operand = std::iter::successors(Some(node), |n| n.parent()).find(|n| {
        n.parent().is_some_and(|p| match p.kind() {
            "boolean_operator" => p.child_by_field_name("right") == Some(*n),
            "conditional_expression" => p.named_child(1) != Some(*n),
            _ => false,
        })
    });
    operand.map(|n| n.byte_range())
}

/// `from m import x` binds `x`; `from m import x as y` binds `y`: (bound, original).
fn from_binding<'a>(imported: tree_sitter::Node, source: &'a str) -> (&'a str, &'a str) {
    let original = imported.child_by_field_name("name").unwrap_or(imported);
    let bound = imported.child_by_field_name("alias").unwrap_or(original);
    (&source[bound.byte_range()], &source[original.byte_range()])
}

/// `import a.b` binds `a`; `import a.b as c` binds `c` to `a.b`: (bound, module).
fn module_binding<'a>(imported: tree_sitter::Node, source: &'a str) -> (&'a str, &'a str) {
    match imported.child_by_field_name("alias") {
        Some(alias) => (
            &source[alias.byte_range()],
            imported
                .child_by_field_name("name")
                .map_or("", |m| &source[m.byte_range()]),
        ),
        None => {
            let first = source[imported.byte_range()].split('.').next().unwrap_or("");
            (first, first)
        }
    }
}

/// The bindings of one name in one scope, in source order.
#[derive(Default)]
struct Bindings {
    /// Where each binding takes effect, and whether it binds what we want.
    events: Vec<(usize, bool)>,
    /// Whether any binding sits in a branch, loop or handler, so the paths to a call matter.
    branched: bool,
    /// `global name`: the scope does not bind it; the module does.
    global: bool,
    /// `nonlocal name`: the scope does not bind it; an enclosing function does.
    nonlocal: bool,
    /// Bindings by walruses that may be skipped: `a and (cast := f)`, `x if (cast := f) else y`.
    /// Each one's innermost operand that may be skipped.
    maybe: HashMap<usize, std::ops::Range<usize>>,
    /// When `branched`: what the name may hold entering each statement, by tree-sitter id,
    /// from one pass over the scope, so each call follows only its own statement.
    entries: std::cell::OnceCell<HashMap<usize, u8>>,
}

/// What a name may hold at a point, one bit per possibility.
const TYPING: u8 = 1;
const OTHER: u8 = 2;
const UNBOUND: u8 = 4;

fn holds(typing: bool) -> u8 {
    if typing { TYPING } else { OTHER }
}

/// What leaves a compound statement normally, by `break`, `continue` and `return`, and what
/// reaches each enclosing suppressing `with` (which are fixed by where the statement is).
type Summary = (u8, u8, u8, u8, Vec<u8>);

/// What a statement may raise.
#[derive(Clone, Copy)]
enum Raises<'a> {
    Nothing,
    Only(&'a str),
    /// The type or any subclass: reading a name raises `NameError` or `UnboundLocalError`.
    Under(&'a str),
    Anything,
}

/// A literal: `True`, `1`, `"text"`, which evaluates and tests without running any code.
fn literal(node: tree_sitter::Node) -> bool {
    match node.kind() {
        "integer" | "float" | "true" | "false" | "none" => true,
        "string" => !named_children(node).iter().any(|c| c.kind() == "interpolation"),
        "parenthesized_expression" => named_children(node).into_iter().all(literal),
        _ => false,
    }
}

/// Whether evaluating `node` reads names and literals only, so raises at most `NameError`.
fn only_names(node: tree_sitter::Node) -> bool {
    match node.kind() {
        "identifier" | "integer" | "float" | "true" | "false" | "none" | "comment" => true,
        "string" => !named_children(node).iter().any(|c| c.kind() == "interpolation"),
        "expression_statement" | "assignment" | "named_expression" | "parenthesized_expression" => {
            named_children(node).into_iter().all(only_names)
        }
        _ => false,
    }
}

/// Python's built-in names besides its exceptions: reading one never raises.
// Assumes they keep their meaning.
const BUILTINS: &[&str] = &[
    "abs",
    "aiter",
    "all",
    "anext",
    "any",
    "ascii",
    "bin",
    "bool",
    "breakpoint",
    "bytearray",
    "bytes",
    "callable",
    "chr",
    "classmethod",
    "compile",
    "complex",
    "delattr",
    "dict",
    "dir",
    "divmod",
    "enumerate",
    "eval",
    "exec",
    "filter",
    "float",
    "format",
    "frozenset",
    "getattr",
    "globals",
    "hasattr",
    "hash",
    "help",
    "hex",
    "id",
    "input",
    "int",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "list",
    "locals",
    "map",
    "max",
    "memoryview",
    "min",
    "next",
    "object",
    "oct",
    "open",
    "ord",
    "pow",
    "print",
    "property",
    "range",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "slice",
    "sorted",
    "staticmethod",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "vars",
    "zip",
    "__import__",
    "Ellipsis",
    "NotImplemented",
    "BaseException",
    "IOError",
    "EnvironmentError",
];

/// Whether a call's arguments are literals, which evaluate without raising.
fn constant_arguments(call: tree_sitter::Node) -> bool {
    let args = call
        .child_by_field_name("arguments")
        .map(named_children)
        .unwrap_or_default();
    args.into_iter().all(|a| match a.kind() {
        "string" => !named_children(a).iter().any(|c| c.kind() == "interpolation"),
        kind => matches!(kind, "integer" | "float" | "true" | "false" | "none" | "comment"),
    })
}

/// `Ok`: what the name may hold after a statement. `Err`: what it may hold at the call, which
/// the statement contains; `?` carries it out.
type Flow = Result<u8, u8>;

/// Follows the paths through one scope's statements to the call at `call`.
struct Paths<'b> {
    lookup: &'b Lookup<'b>,
    events: &'b [(usize, bool)],
    maybe: &'b HashMap<usize, std::ops::Range<usize>>,
    call: usize,
    /// What the name may hold entering each statement passed.
    entries: HashMap<usize, u8>,
    /// What the innermost loop's `break`s carry past it, and its `continue`s back to its head.
    breaks: u8,
    continues: u8,
    /// What pending `return`s carry: an enclosing `finally` runs on them, and may `break`.
    returns: u8,
    /// `summary`'s answers, by statement and entry state.
    summaries: HashMap<(usize, u8), Summary>,
    /// The bindings of other names read where an exception may be swallowed, by scope and name.
    locals: HashMap<(usize, &'b str), Bindings>,
    /// Whether to route exceptions to the handlers and managers that may catch them. Off, any
    /// handler may start anywhere in its body and a suppressing `with` lets anything out: for
    /// asking whether another name is bound, which would otherwise ask again without end.
    routed: bool,
    /// The enclosing `with` statements that swallow exceptions, innermost last: the types each
    /// swallows, and what the name may hold where the body raised one.
    /// A `try` body's handlers sit here too, their states unused, so what they surely catch
    /// goes no further out.
    swallowing: Vec<(Vec<&'b str>, u8)>,
}

impl<'b> Paths<'b> {
    fn contains(&self, node: tree_sitter::Node) -> bool {
        node.byte_range().contains(&self.call)
    }

    /// `state` after the bindings within `node` at or before `limit`.
    fn through(&self, node: tree_sitter::Node, state: u8, limit: usize) -> u8 {
        let end = node.end_byte().min(limit);
        let within = self
            .events
            .iter()
            .filter(|(at, _)| (node.start_byte()..=end).contains(at));
        within.fold(state, |state, (at, typing)| {
            // Skippable within `node`; when `node` is the operand itself, `test` has split it.
            let range = node.byte_range();
            let inside = |r: &std::ops::Range<usize>| *r != range && range.start <= r.start && r.end <= range.end;
            let skippable = if self.maybe.get(at).is_some_and(inside) {
                state
            } else {
                0
            };
            skippable | holds(*typing)
        })
    }

    /// Every value bound within `node`.
    fn all(&self, node: tree_sitter::Node) -> u8 {
        let within = self.events.iter().filter(|(at, _)| node.byte_range().contains(at));
        within.fold(0, |all, (_, typing)| all | holds(*typing))
    }

    fn part(&mut self, node: Option<tree_sitter::Node>, state: u8) -> Flow {
        match node {
            None => Ok(state),
            Some(n) if matches!(n.kind(), "block" | "module") => self.block(n, state),
            Some(n) => self.statement(n, state),
        }
    }

    /// Records what the name may hold where `node` raises, for each enclosing `with` that may
    /// swallow it: entering `node`, or after any binding within it.
    fn raising(&mut self, node: tree_sitter::Node, state: u8) {
        if self.swallowing.is_empty() {
            return;
        }
        let raised = self.raised(node);
        let held = state | self.all(node);
        // Innermost first: what a manager surely swallows goes no further out.
        for (types, at) in self.swallowing.iter_mut().rev() {
            let (may, must) = match raised {
                Raises::Nothing => (false, false),
                Raises::Only(name) => (
                    types.iter().any(|t| may_catch(t, name)),
                    types.iter().any(|t| must_catch(t, Some(name))),
                ),
                // A handler for the type, a base of it, or a subclass of it may catch.
                Raises::Under(name) => (
                    types.iter().any(|t| may_catch(t, name) || may_catch(name, t)),
                    types.iter().any(|t| must_catch(t, Some(name))),
                ),
                Raises::Anything => (true, types.iter().any(|t| must_catch(t, None))),
            };
            if may {
                *at |= held;
            }
            if must {
                break;
            }
        }
    }

    /// What `node` itself may raise, apart from the statements in its blocks: `pass`,
    /// `x = str` or `while True:` nothing, `x = y` only `NameError`, `raise KeyError("…")`
    /// only that.
    fn raised(&mut self, node: tree_sitter::Node) -> Raises<'b> {
        let source = self.lookup.source;
        let mut cursor = node.walk();
        let elifs = node.children_by_field_name("alternative", &mut cursor);
        let conditions: Vec<_> = elifs.filter_map(|e| e.child_by_field_name("condition")).collect();
        match node.kind() {
            "pass_statement" | "break_statement" | "continue_statement" | "global_statement" | "nonlocal_statement"
            | "try_statement" => Raises::Nothing,
            // `if x:` calls `x.__bool__()`, which may raise; `while True:` tests a literal.
            "if_statement" | "while_statement"
                if node
                    .child_by_field_name("condition")
                    .into_iter()
                    .chain(conditions.iter().copied())
                    .all(literal) =>
            {
                Raises::Nothing
            }
            // `cast = str` reads a built-in; `cast = optional_cast` may raise `NameError`.
            _ if only_names(node) && self.reads_bound(node) => Raises::Nothing,
            _ if only_names(node) => Raises::Under("NameError"),
            // `with suppress(KeyError):` builds a known manager from names: nothing.
            "with_statement" if self.lookup.suppressing.contains_key(&node.id()) && self.plain_manager(node) => {
                Raises::Nothing
            }
            "raise_statement" if node.child_by_field_name("cause").is_none() => {
                let exception = node.named_child(0);
                // `raise E` or `raise E("literal")`: evaluating it raises nothing else.
                let class = exception.and_then(|e| match e.kind() {
                    "identifier" => Some(e),
                    "call" if constant_arguments(e) => e.child_by_field_name("function"),
                    _ => None,
                });
                class
                    .filter(|c| c.kind() == "identifier")
                    .map_or(Raises::Anything, |c| Raises::Only(&source[c.byte_range()]))
            }
            _ => Raises::Anything,
        }
    }

    /// Whether every name `node` reads is bound there, so reading it cannot raise `NameError`.
    fn reads_bound(&mut self, node: tree_sitter::Node) -> bool {
        let read = match node.kind() {
            "identifier" => {
                let name = &self.lookup.source[node.byte_range()];
                return self.bound(node, name);
            }
            // The target is stored, not read.
            "assignment" => node.child_by_field_name("right"),
            "named_expression" => node.child_by_field_name("value"),
            _ => None,
        };
        let parts = read.map_or_else(|| named_children(node), |r| vec![r]);
        parts.into_iter().all(|p| self.reads_bound(p))
    }

    /// Whether `name` is bound where `node` reads it: a parameter or a definitely assigned
    /// name of its scope; in a function, a name an enclosing function or the module binds; else
    /// a built-in the function does not make its own local.
    fn bound(&mut self, node: tree_sitter::Node, name: &'b str) -> bool {
        let builtin = BUILTINS.contains(&name) || EXCEPTIONS.iter().any(|(n, _)| *n == name);
        let scopes = std::iter::successors(node.parent(), |n| n.parent());
        let mut scopes = scopes.filter_map(|s| Some((s, scope_body(s)?)));
        let Some((scope, body)) = scopes.find(|(_, b)| b.byte_range().contains(&node.start_byte())) else {
            return builtin;
        };
        let lookup = Lookup { name, ..*self.lookup };
        let bindings = self
            .locals
            .entry((scope.id(), name))
            .or_insert_with(|| lookup.scan(body));
        if lookup.held(scope, body, bindings, node, false, false) & UNBOUND == 0 {
            return true;
        }
        // Unbound here: a function's own local raises.
        if lookup.function_local(scope, bindings) {
            return false;
        }
        // A function runs after the code around it has: a name an enclosing function or the
        // module binds is there by then. A class body's names are not seen from its methods.
        // Assumes a function is called after the module binds the names it reads.
        let function = matches!(scope.kind(), "function_definition" | "lambda");
        let outer = std::iter::successors(scope.parent(), |n| n.parent())
            .filter(|s| s.kind() != "class_definition")
            .filter_map(|s| Some((s, scope_body(s)?)))
            .any(|(s, b)| {
                let bindings = self.locals.entry((s.id(), name)).or_insert_with(|| lookup.scan(b));
                lookup.is_parameter(s) || !bindings.events.is_empty()
            });
        builtin || (function && outer)
    }

    /// Whether a suppressing `with` has one manager, called with names and literals only.
    fn plain_manager(&self, with: tree_sitter::Node) -> bool {
        let header = named_children(with).into_iter().find(|c| c.kind() == "with_clause");
        let items = header.map(named_children).unwrap_or_default();
        let [item] = items.as_slice() else {
            return false;
        };
        let value = item.child_by_field_name("value");
        let call = value.map(|v| {
            if v.kind() == "as_pattern" {
                v.named_child(0).unwrap_or(v)
            } else {
                v
            }
        });
        let args = call
            .and_then(|c| c.child_by_field_name("arguments"))
            .map(named_children);
        args.unwrap_or_default().into_iter().all(|a| {
            let value = if a.kind() == "keyword_argument" {
                a.child_by_field_name("value")
            } else {
                Some(a)
            };
            value.is_some_and(|v| {
                only_names(v)
                    || matches!(v.kind(), "attribute" | "tuple") && named_children(v).into_iter().all(only_names)
            })
        })
    }

    /// What the name may hold after `node` tests true, and after it tests false: `a and
    /// (cast := f)` binds on the true path only.
    fn test(&mut self, node: Option<tree_sitter::Node>, state: u8) -> Result<(u8, u8), u8> {
        let Some(node) = node else {
            return Ok((state, state));
        };
        let field = |name| node.child_by_field_name(name);
        match node.kind() {
            "if_clause" | "parenthesized_expression" if node.named_child_count() == 1 => {
                self.test(node.named_child(0), state)
            }
            "not_operator" => self.test(field("argument"), state).map(|(t, f)| (f, t)),
            // `a if c else b`: `c` runs first, then one alternative.
            "conditional_expression" => {
                let (taken, skipped) = self.test(node.named_child(1), state)?;
                let (a_true, a_false) = self.test(node.named_child(0), taken)?;
                let (b_true, b_false) = self.test(node.named_child(2), skipped)?;
                Ok((a_true | b_true, a_false | b_false))
            }
            "boolean_operator" => {
                let and = field("operator").is_some_and(|o| o.kind() == "and");
                let (left_true, left_false) = self.test(field("left"), state)?;
                let (right_true, right_false) = self.test(field("right"), if and { left_true } else { left_false })?;
                Ok(if and {
                    (right_true, left_false | right_false)
                } else {
                    (left_true | right_true, right_false)
                })
            }
            // Any other expression holding one: its parts, in order.
            _ if self.splits(node) => {
                let mut state = state;
                for part in named_children(node) {
                    let (t, f) = self.test(Some(part), state)?;
                    state = t | f;
                }
                // Its own binding, last: `cast = a if b else (cast := c)` assigns after both.
                let own = self
                    .events
                    .iter()
                    .rfind(|(at, _)| *at == node.end_byte() && !self.maybe.contains_key(at));
                if let Some((_, typing)) = own.filter(|_| state != 0) {
                    state = holds(*typing);
                }
                Ok((state, state))
            }
            _ => self.expression(node, state).map(|s| (s, s)),
        }
    }

    fn block(&mut self, node: tree_sitter::Node, mut state: u8) -> Flow {
        for statement in named_children(node) {
            *self.entries.entry(statement.id()).or_default() |= state;
            state = self.statement(statement, state)?;
        }
        Ok(state)
    }

    fn statement(&mut self, node: tree_sitter::Node, state: u8) -> Flow {
        // No path reaches here (after `break`, `return`…): what it binds reaches nothing either.
        if state == 0 {
            return if self.contains(node) { Err(0) } else { Ok(0) };
        }
        self.raising(node, state);
        let compound = matches!(
            node.kind(),
            "if_statement"
                | "for_statement"
                | "while_statement"
                | "try_statement"
                | "with_statement"
                | "match_statement"
        );
        if compound {
            return if self.contains(node) {
                self.compound(node, state)
            } else {
                Ok(self.summary(node, state))
            };
        }
        match node.kind() {
            "return_statement" if !self.contains(node) => {
                self.returns |= state;
                Ok(0)
            }
            "raise_statement" if !self.contains(node) => Ok(0),
            "break_statement" => {
                self.breaks |= state;
                Ok(0)
            }
            "continue_statement" => {
                self.continues |= state;
                Ok(0)
            }
            _ => self.expression(node, state),
        }
    }

    /// What the name may hold after evaluating `node`, or at the call within it.
    fn expression(&mut self, node: tree_sitter::Node, state: u8) -> Flow {
        if state == 0 {
            return if self.contains(node) { Err(0) } else { Ok(0) };
        }
        if self.splits(node) {
            return self.test(Some(node), state).map(|(t, f)| t | f);
        }
        if self.contains(node) {
            Err(self.through(node, state, self.call))
        } else {
            Ok(self.through(node, state, usize::MAX))
        }
    }

    /// Whether `node` holds a binding that may be skipped within it: `a and (cast := f)`.
    fn splits(&self, node: tree_sitter::Node) -> bool {
        let range = node.byte_range();
        self.maybe
            .values()
            .any(|r| *r != range && range.start <= r.start && r.end <= range.end)
    }

    fn compound(&mut self, node: tree_sitter::Node, state: u8) -> Flow {
        match node.kind() {
            "if_statement" => self.branches(node, state),
            "for_statement" | "while_statement" => self.looped(node, state),
            "try_statement" => self.tried(node, state),
            "with_statement" => {
                let header = named_children(node).into_iter().find(|c| c.kind() == "with_clause");
                // What the header raises is the `with` statement's, recorded with it.
                let entered = match header {
                    Some(h) => self.expression(h, state)?,
                    None => state,
                };
                let body = node.child_by_field_name("body");
                // A manager that swallows exceptions lets out what the name held wherever the
                // body raised one.
                let Some(types) = self.lookup.suppressing.get(&node.id()) else {
                    return self.part(body, entered);
                };
                if !self.routed {
                    return Ok(self.part(body, entered)? | entered | body.map_or(0, |b| self.all(b)));
                }
                self.swallowing.push((types.clone(), 0));
                let done = self.part(body, entered);
                let (_, raised) = self.swallowing.pop().expect("pushed above");
                Ok(done? | raised)
            }
            _ => self.matched(node, state),
        }
    }

    /// What leaves a compound statement the call is not in, and what its `break`s,
    /// `continue`s and `return`s carry: worked out once per entry state, as loops and
    /// `finally` ask again and again, so nesting stays linear.
    fn summary(&mut self, node: tree_sitter::Node, state: u8) -> u8 {
        let key = (node.id(), state);
        let known = match self.summaries.get(&key) {
            Some(known) => known.clone(),
            None => {
                let outer = (
                    std::mem::take(&mut self.breaks),
                    std::mem::take(&mut self.continues),
                    std::mem::take(&mut self.returns),
                );
                let raised: Vec<u8> = self.swallowing.iter_mut().map(|(_, at)| std::mem::take(at)).collect();
                let out = self.compound(node, state).unwrap_or_else(|at| at);
                let delta = self
                    .swallowing
                    .iter_mut()
                    .zip(&raised)
                    .map(|((_, at), before)| std::mem::replace(at, *before));
                let known = (
                    out,
                    std::mem::replace(&mut self.breaks, outer.0),
                    std::mem::replace(&mut self.continues, outer.1),
                    std::mem::replace(&mut self.returns, outer.2),
                    delta.collect(),
                );
                self.summaries.insert(key, known.clone());
                known
            }
        };
        let (out, breaks, continues, returns, raised) = known;
        self.breaks |= breaks;
        self.continues |= continues;
        self.returns |= returns;
        for ((_, at), delta) in self.swallowing.iter_mut().zip(raised) {
            *at |= delta;
        }
        out
    }

    fn branches(&mut self, node: tree_sitter::Node, state: u8) -> Flow {
        // `otherwise`: the state when no branch so far was taken.
        let (taken, mut otherwise) = self.test(node.child_by_field_name("condition"), state)?;
        let mut out = self.part(node.child_by_field_name("consequence"), taken)?;
        let mut cursor = node.walk();
        let alternatives: Vec<_> = node.children_by_field_name("alternative", &mut cursor).collect();
        for alternative in alternatives {
            if alternative.kind() == "else_clause" {
                out |= self.part(alternative.child_by_field_name("body"), otherwise)?;
                otherwise = 0;
            } else {
                let (taken, rest) = self.test(alternative.child_by_field_name("condition"), otherwise)?;
                out |= self.part(alternative.child_by_field_name("consequence"), taken)?;
                otherwise = rest;
            }
        }
        Ok(out | otherwise)
    }

    /// A body that runs any number of times: what reaches its head is what enters plus what
    /// any pass leaves, found by repeating until nothing new arrives.
    fn looped(&mut self, node: tree_sitter::Node, state: u8) -> Flow {
        let iterable = self.part(node.child_by_field_name("right"), state)?;
        let condition = node.child_by_field_name("condition");
        let body = node.child_by_field_name("body");
        let target = node.kind() == "for_statement" && self.lookup.binds(node).is_some();
        let enter = |head: u8| if target { OTHER } else { head };
        let outer = (std::mem::take(&mut self.breaks), std::mem::take(&mut self.continues));
        let call = std::mem::replace(&mut self.call, usize::MAX);
        let mut head = iterable;
        loop {
            let (tested, _) = self.test(condition, head).unwrap_or_else(|at| (at, at));
            let passed = self.part(body, enter(tested)).unwrap_or_else(|at| at);
            let next = head | passed | std::mem::take(&mut self.continues);
            if next == head {
                break;
            }
            head = next;
        }
        self.call = call;
        let finished = self
            .test(condition, head)
            .and_then(|(tested, failed)| self.part(body, enter(tested)).map(|_| failed));
        let breaks = std::mem::replace(&mut self.breaks, outer.0);
        self.continues = outer.1;
        // `while True:` leaves only by `break`.
        let forever = condition.is_some_and(|c| matches!(&self.lookup.source[c.byte_range()], "True" | "1"));
        let finished = if forever { finished.map(|_| 0)? } else { finished? };
        // A `break` skips the loop's `else`.
        let otherwise = node.child_by_field_name("alternative");
        Ok(self.part(otherwise.and_then(|e| e.child_by_field_name("body")), finished)? | breaks)
    }

    fn tried(&mut self, node: tree_sitter::Node, state: u8) -> Flow {
        let outer = (
            std::mem::take(&mut self.breaks),
            std::mem::take(&mut self.continues),
            std::mem::take(&mut self.returns),
        );
        // An exception leaving for an enclosing suppressing `with` runs `finally` on the way.
        let finally_clause = named_children(node).into_iter().find(|c| c.kind() == "finally_clause");
        let finally_block = finally_clause.and_then(|c| named_children(c).into_iter().find(|b| b.kind() == "block"));
        let leaving: Vec<u8> = match finally_block {
            Some(_) => self.swallowing.iter_mut().map(|(_, at)| std::mem::take(at)).collect(),
            None => vec![],
        };
        let guarded = self.guarded(node, state);
        for (level, before) in leaving.into_iter().enumerate() {
            let left = std::mem::replace(&mut self.swallowing[level].1, before);
            if left != 0 {
                let after = self.quietly(finally_block, left);
                self.swallowing[level].1 |= after;
            }
        }
        let (out, raised, finally) = guarded?;
        let breaks = std::mem::replace(&mut self.breaks, outer.0);
        let continues = std::mem::replace(&mut self.continues, outer.1);
        let returns = std::mem::replace(&mut self.returns, outer.2);
        // `finally` runs on every way out, carrying what each leaves with. A `break` or
        // `continue` in it redirects a pending `return` or exception to the loop.
        self.breaks |= self.quietly(finally, breaks);
        self.continues |= self.quietly(finally, continues);
        self.returns |= self.quietly(finally, returns);
        self.quietly(finally, raised);
        let entry = if finally.is_some_and(|f| self.contains(f)) {
            out | raised | breaks | continues | returns
        } else {
            out
        };
        self.part(finally, entry)
    }

    /// A `try`'s body, handlers and `else`: what leaves them normally, what an exception may
    /// carry from the body, and the `finally` block.
    fn guarded<'t>(
        &mut self,
        node: tree_sitter::Node<'t>,
        state: u8,
    ) -> Result<(u8, u8, Option<tree_sitter::Node<'t>>), u8> {
        let body = node.child_by_field_name("body");
        // Each handler collects what the name held where the body raised what it may catch;
        // the first sits innermost, so what it surely catches reaches no later one, nor an
        // enclosing suppressing `with`.
        let handlers: Vec<_> = named_children(node)
            .into_iter()
            .filter(|c| matches!(c.kind(), "except_clause" | "except_group_clause"))
            .collect();
        let depth = self.swallowing.len();
        for clause in handlers.iter().rev().filter(|_| self.routed) {
            self.swallowing.push((handler_types(*clause, self.lookup.source), 0));
        }
        let done = self.part(body, state);
        let mut caught: Vec<u8> = self.swallowing.drain(depth..).map(|(_, at)| at).collect();
        let done = done?;
        // A `finally` may start from any point in the body.
        let raised = state | done | body.map_or(0, |b| self.all(b));
        if !self.routed {
            caught = vec![raised; handlers.len()];
        }
        let (mut normal, mut handled, mut finally) = (done, 0, None);
        for clause in named_children(node) {
            let block = named_children(clause).into_iter().find(|c| c.kind() == "block");
            match clause.kind() {
                "except_clause" | "except_group_clause" => {
                    let reaching = caught.pop().unwrap_or(0);
                    // `except E as cast:` binds in the clause's header, before its block.
                    let header_end = block.map_or(clause.end_byte(), |b| b.start_byte());
                    let entry = if reaching == 0 {
                        0
                    } else {
                        self.through(clause, reaching, header_end.saturating_sub(1))
                    };
                    handled |= self.part(block, entry)?;
                }
                "else_clause" => normal = self.part(clause.child_by_field_name("body"), done)?,
                "finally_clause" => finally = block,
                _ => {}
            }
        }
        Ok((normal | handled, raised, finally))
    }

    /// What leaves `node` from `state`, ignoring the call: for paths found after the fact.
    /// `case _:` or `case name:`: the pattern matches whatever reaches it.
    fn irrefutable(&self, case: tree_sitter::Node) -> bool {
        let patterns: Vec<_> = named_children(case)
            .into_iter()
            .filter(|c| c.kind() == "case_pattern")
            .collect();
        let [pattern] = patterns.as_slice() else {
            return false;
        };
        // `_` parses as an empty pattern; a capture as a one-part name. `True` and `1` are literals.
        let wildcard = || pattern.named_child_count() == 0 && &self.lookup.source[pattern.byte_range()] == "_";
        let capture = || {
            let inner = named_children(*pattern);
            matches!(inner.as_slice(), [name] if name.kind() == "dotted_name" && name.named_child_count() == 1)
        };
        wildcard() || capture()
    }

    fn quietly(&mut self, node: Option<tree_sitter::Node>, state: u8) -> u8 {
        let call = std::mem::replace(&mut self.call, usize::MAX);
        let out = self.part(node, state).unwrap_or_else(|at| at);
        self.call = call;
        out
    }

    fn matched(&mut self, node: tree_sitter::Node, state: u8) -> Flow {
        let subject = self.part(node.child_by_field_name("subject"), state)?;
        let cases = node.child_by_field_name("body").map(named_children).unwrap_or_default();
        // `pending`: what reaches the next case, having failed every case so far.
        let (mut pending, mut out) = (subject, 0);
        for case in cases {
            let captured = pending != 0 && self.lookup.binds(case).is_some();
            let entry = if captured { OTHER } else { pending };
            // `case 1 if (cast := str):` binds in its guard, before the body, and a failed
            // guard carries that binding on to the next case.
            let guard = case.child_by_field_name("guard");
            let (passed, failed) = self.test(guard, entry)?;
            out |= self.part(case.child_by_field_name("consequence"), passed)?;
            pending = match (self.irrefutable(case), guard.is_some()) {
                (true, false) => 0,
                (true, true) => failed,
                (false, _) => pending | failed,
            };
        }
        // With no case matching, what reached the end passes through.
        Ok(out | pending)
    }
}

struct Lookup<'a> {
    name: &'a str,
    want: Want,
    source: &'a str,
    /// `Typing::suppressing`.
    suppressing: &'a Swallows<'a>,
}

impl Lookup<'_> {
    fn text(&self, node: tree_sitter::Node) -> &str {
        &self.source[node.byte_range()]
    }

    fn is_name(&self, node: Option<tree_sitter::Node>) -> bool {
        node.is_some_and(|n| self.text(n) == self.name)
    }

    fn is_parameter(&self, function: tree_sitter::Node) -> bool {
        let params = function
            .child_by_field_name("parameters")
            .map(named_children)
            .unwrap_or_default();
        params.into_iter().any(|p| {
            let ident = if p.kind() == "identifier" {
                Some(p)
            } else {
                p.child_by_field_name("name").or_else(|| p.named_child(0))
            };
            self.is_name(ident)
        })
    }

    /// What the name may hold in `scope` when read at `at`, or at `at` or `later`. UNBOUND
    /// means the enclosing scope decides.
    fn reach(
        &self,
        scope: tree_sitter::Node,
        body: tree_sitter::Node,
        bindings: &Bindings,
        point: tree_sitter::Node,
        later: bool,
    ) -> u8 {
        let mut reach = self.held(scope, body, bindings, point, later, true);
        // A name a function binds anywhere is local to all of it: unbound, it is an error to
        // read, not the enclosing scope's. `global` and `nonlocal` hand it outward.
        if self.function_local(scope, bindings) {
            reach &= !UNBOUND;
        }
        reach
    }

    fn function_local(&self, scope: tree_sitter::Node, bindings: &Bindings) -> bool {
        let shared = matches!(scope.kind(), "module" | "class_definition");
        let local = self.is_parameter(scope) || !bindings.events.is_empty();
        !shared && local && !bindings.global && !bindings.nonlocal
    }

    /// What the name may hold in `scope` at `point` (or later), UNBOUND where nothing in the
    /// scope bound it yet.
    fn held(
        &self,
        scope: tree_sitter::Node,
        body: tree_sitter::Node,
        bindings: &Bindings,
        point: tree_sitter::Node,
        later: bool,
        routed: bool,
    ) -> u8 {
        let at = point.start_byte();
        let parameter = self.is_parameter(scope);
        let entry = if parameter { OTHER } else { UNBOUND };
        let straight = || {
            let last = bindings.events.iter().rev().find(|(pos, _)| *pos <= at);
            last.map_or(entry, |(_, typing)| holds(*typing))
        };
        let mut reach = if bindings.branched {
            let mut paths = Paths {
                lookup: self,
                events: &bindings.events,
                maybe: &bindings.maybe,
                call: usize::MAX,
                entries: HashMap::new(),
                breaks: 0,
                continues: 0,
                returns: 0,
                summaries: HashMap::new(),
                swallowing: vec![],
                locals: HashMap::new(),
                routed,
            };
            let entries = bindings.entries.get_or_init(|| {
                let _ = paths.part(Some(body), entry);
                std::mem::take(&mut paths.entries)
            });
            // From the innermost statement holding the point, or the whole body if none does.
            let (start, state) = std::iter::successors(Some(point), |n| n.parent())
                .find_map(|n| Some((n, *entries.get(&n.id())?)))
                .unwrap_or((body, entry));
            paths.call = at;
            paths.part(Some(start), state).err().unwrap_or_else(straight)
        } else {
            straight()
        };
        if later {
            let after = bindings.events.iter().filter(|(pos, _)| *pos > at);
            reach |= after.fold(0, |all, (_, typing)| all | holds(*typing));
        }
        reach
    }

    fn scan(&self, scope: tree_sitter::Node) -> Bindings {
        let mut out = Bindings::default();
        self.visit(scope, true, &mut out);
        out.events.sort_unstable_by_key(|(at, _)| *at);
        out
    }

    /// Every binding under `node` that belongs to this scope: nested functions, classes,
    /// lambdas and comprehensions have their own.
    /// `top`: whether `node` runs every time its scope does, outside any branch, loop or handler.
    fn visit(&self, node: tree_sitter::Node, top: bool, out: &mut Bindings) {
        for child in named_children(node) {
            if let Some(typing) = self.binds(child) {
                out.events.push((bound_at(child), typing));
                let operand = (child.kind() == "named_expression")
                    .then(|| skippable_operand(child))
                    .flatten();
                let skippable = operand.is_some();
                if let Some(range) = operand {
                    out.maybe.insert(bound_at(child), range);
                }
                out.branched |= !top || skippable || matches!(child.kind(), "for_statement" | "except_clause");
            }
            let declares = || named_children(child).into_iter().any(|n| self.is_name(Some(n)));
            match child.kind() {
                "function_definition" | "class_definition" | "decorated_definition" | "lambda" => {}
                "list_comprehension" | "set_comprehension" | "dictionary_comprehension" | "generator_expression" => {}
                "global_statement" => out.global |= declares(),
                "nonlocal_statement" => out.nonlocal |= declares(),
                "block" => self.visit(child, false, out),
                _ => self.visit(child, top, out),
            }
        }
    }

    /// Whether this node binds the name, and if so whether to what we want.
    fn binds(&self, node: tree_sitter::Node) -> Option<bool> {
        match node.kind() {
            "import_from_statement" => self.import_from(node),
            "import_statement" => self.module_import(node),
            "function_definition" | "class_definition" => {
                self.is_name(node.child_by_field_name("name")).then_some(false)
            }
            "decorated_definition" => self
                .is_name(
                    node.child_by_field_name("definition")
                        .and_then(|d| d.child_by_field_name("name")),
                )
                .then_some(false),
            "assignment" | "augmented_assignment" | "for_statement" | "for_in_clause" => {
                self.target(node.child_by_field_name("left")).then_some(false)
            }
            "named_expression" => self.is_name(node.child_by_field_name("name")).then_some(false),
            "case_clause" => named_children(node)
                .into_iter()
                .any(|c| c.kind() == "case_pattern" && self.captures(c))
                .then_some(false),
            "as_pattern" | "except_clause" => self.target(node.child_by_field_name("alias")).then_some(false),
            _ => None,
        }
    }

    /// Whether a `case` pattern captures the name: `case cast:`, `{"k": cast}`, `[*cast]`,
    /// `{**cast}`, `X(a=cast)` or `… as cast`; not a class name or a dotted value like `C.cast`.
    fn captures(&self, node: tree_sitter::Node) -> bool {
        let children = named_children(node);
        match node.kind() {
            "dotted_name" => {
                let bare = children.len() == 1 && node.parent().is_none_or(|p| p.kind() != "class_pattern");
                bare && self.is_name(Some(node))
            }
            "splat_pattern" | "as_pattern" => children.into_iter().any(|c| {
                if c.kind() == "identifier" {
                    self.is_name(Some(c))
                } else {
                    self.captures(c)
                }
            }),
            "keyword_pattern" => children.into_iter().skip(1).any(|c| self.captures(c)),
            _ => children.into_iter().any(|c| self.captures(c)),
        }
    }

    /// An assignment target that binds the name: `cast`, `a, cast`, `(cast, b)`; not `x.cast`.
    fn target(&self, node: Option<tree_sitter::Node>) -> bool {
        let Some(node) = node else {
            return false;
        };
        match node.kind() {
            "identifier" => self.text(node) == self.name,
            "as_pattern_target"
            | "pattern_list"
            | "tuple_pattern"
            | "list_pattern"
            | "tuple"
            | "list"
            | "parenthesized_expression"
            | "list_splat_pattern" => named_children(node).into_iter().any(|c| self.target(Some(c))),
            _ => false,
        }
    }

    /// `from m import cast` or `from m import x as cast`.
    fn import_from(&self, statement: tree_sitter::Node) -> Option<bool> {
        let module = statement
            .child_by_field_name("module_name")
            .map_or("", |m| self.text(m));
        let mut cursor = statement.walk();
        statement
            .children_by_field_name("name", &mut cursor)
            .filter_map(|imported| {
                let (bound, original) = from_binding(imported, self.source);
                let wanted =
                    matches!(self.want, Want::Name(api) if api.modules().contains(&module) && original == api.name());
                (bound == self.name).then_some(wanted)
            })
            .last()
    }

    /// `import typing` binds `typing`; `import typing as t` binds `t`.
    fn module_import(&self, statement: tree_sitter::Node) -> Option<bool> {
        let mut cursor = statement.walk();
        statement
            .children_by_field_name("name", &mut cursor)
            .filter_map(|imported| {
                let (bound, module) = module_binding(imported, self.source);
                let wanted = matches!(self.want, Want::Module(api) if api.modules().contains(&module));
                (bound == self.name).then_some(wanted)
            })
            .last()
    }
}
