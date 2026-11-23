//! Rust: every function's qualified name, cyclomatic and cognitive complexity, and the override
//! attributes and comments on it, from one `syn` parse. No external tool is involved.
//!
//! Names:
//!   parse_order                    free function
//!   Order::total                   method in `impl Order`
//!   <Order as Display>::fmt        method in `impl Display for Order`
//!   Validate::check                default method in `trait Validate`
//!   billing::charge                function in an inline `mod billing { … }`
//!   charge>retry                   function declared inside `charge`
//!   charge>items.iter().map()      closure passed to a call
//!   charge>on_error                closure assigned with `let on_error = |…| …`
//! A repeated name in one file gets #2, #3, … in source order.
//!
//! Cyclomatic complexity, the modified variant used for JavaScript: 1, plus each `if` and
//! `if let`, `while` and `while let`, `for`, `match` (one per match), match guard, `let … else`,
//! `&&` and `||`. `?` does not count: error propagation is not a decision in the function.
//!
//! Cognitive complexity, after SonarSource's specification:
//! - `if`, `match` and loops cost 1 plus the current nesting, and nest.
//! - `else if` and `else` cost 1, and nest.
//! - `let … else` costs 1 plus nesting; labelled `break` and `continue` cost 1.
//! - Each run of the same logical operator (`&&`, `||`) costs 1, parentheses or not.
//! - A closure scores separately, starting one level deeper than the code around it.

use std::collections::{HashMap, HashSet};

use proc_macro2::Span;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, BinOp, Block, Expr, ExprClosure, Meta, Pat, Token, Type};

use crate::gate::Metric;
use crate::lang::{MeasuredFunction, NativeAnalysis, NativeOverride, directive_metrics};

pub fn analyse_rust(source: &str) -> NativeAnalysis {
    let file = match syn::parse_file(source) {
        Ok(file) => file,
        Err(e) => {
            return NativeAnalysis {
                error: Some(format!("{e} (line {})", e.span().start().line)),
                ..Default::default()
            };
        }
    };
    let mut analyser = Analyser {
        source,
        lines: source.lines().collect(),
        comment_lines: comment_lines(source),
        ..Default::default()
    };
    // `#![allow(clippy::cognitive_complexity)]` covers the whole file: it approves nothing.
    for attr in &file.attrs {
        if let Some((metric, text)) = attribute_override(attr, source) {
            analyser.overrides.push(NativeOverride {
                line: line_of(attr.span()),
                text,
                metric,
                function: None,
            });
        }
    }
    analyser.visit_file(&file);
    let mut comment_lines: Vec<u32> = analyser.comment_lines.iter().copied().collect();
    comment_lines.sort_unstable();
    analyser.overrides.extend(crate::slop::lint_overrides(
        comment_lines
            .into_iter()
            .map(|n| (n, analyser.lines[n as usize - 1].trim())),
    ));
    NativeAnalysis {
        findings: crate::slop::rust(&file, source, &analyser.comment_lines),
        functions: analyser.functions,
        overrides: analyser.overrides,
        ..Default::default()
    }
}

fn line_of(span: Span) -> u32 {
    span.start().line as u32
}

/// `#[allow(clippy::cognitive_complexity)]` and `#[expect(…)]`, with the attribute's source text.
fn attribute_override(attr: &Attribute, source: &str) -> Option<(Metric, String)> {
    let path = attr.path();
    if !(path.is_ident("allow") || path.is_ident("expect")) {
        return None;
    }
    // The lint list, e.g. `clippy::cognitive_complexity, dead_code, reason = "…"`: only a lint
    // path counts, so a reason that mentions the lint approves nothing.
    let lints = attr
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .ok()?;
    let names_lint = lints.iter().any(|meta| match meta {
        Meta::Path(p) => {
            p.leading_colon.is_none()
                && p.segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .eq(["clippy", "cognitive_complexity"])
        }
        _ => false,
    });
    if !names_lint {
        return None;
    }
    let text = source.get(attr.span().byte_range())?.to_string();
    Some((Metric::Cognitive, text))
}

/// Lines whose first token is a `//` comment. A small lexer: string literals (raw, byte and C
/// strings included), character literals and nested block comments are skipped, so directive
/// text inside them is never mistaken for a comment.
fn comment_lines(source: &str) -> HashSet<u32> {
    lex(source).lines
}

/// A file's comments and string and character literals, for lining up revisions.
pub fn syntax(source: &str) -> crate::lang::Syntax {
    let lexer = lex(source);
    crate::lang::Syntax {
        comments: lexer.spans,
        literals: lexer.literals,
        ..Default::default()
    }
}

fn lex(source: &str) -> Lexer<'_> {
    let mut lexer = Lexer {
        source,
        b: source.as_bytes(),
        i: 0,
        line: 1,
        lines: HashSet::new(),
        spans: Vec::new(),
        literals: Vec::new(),
    };
    let mut blank = true;
    while lexer.i < lexer.b.len() {
        let c = lexer.b[lexer.i];
        if c.is_ascii_whitespace() {
            blank = blank || c == b'\n';
            lexer.step();
            continue;
        }
        lexer.token(c, blank);
        blank = false;
    }
    lexer
}

struct Lexer<'s> {
    source: &'s str,
    b: &'s [u8],
    i: usize,
    line: u32,
    lines: HashSet<u32>,
    spans: Vec<std::ops::Range<usize>>,
    literals: Vec<std::ops::Range<usize>>,
}

impl Lexer<'_> {
    /// Advance one byte, counting lines.
    fn step(&mut self) {
        if self.b[self.i] == b'\n' {
            self.line += 1;
        }
        self.i += 1;
    }

    fn at(&self, text: &[u8]) -> bool {
        self.b[self.i..].starts_with(text)
    }

    /// Skip the token that starts at `c`; `first` when nothing precedes it on its line.
    fn token(&mut self, c: u8, first: bool) {
        match c {
            b'/' if self.at(b"//") => self.line_comment(first),
            b'/' if self.at(b"/*") => self.block_comment(),
            b'"' => self.literal(Self::string),
            b'\'' => self.char_or_lifetime(),
            b'r' | b'b' | b'c' => {
                let start = self.i;
                if self.raw_string() {
                    self.literals.push(start..self.i.min(self.b.len()));
                } else {
                    self.i += 1;
                }
            }
            _ => self.i += 1,
        }
    }

    /// Skip a literal with `skip`, recording where it was.
    fn literal(&mut self, skip: fn(&mut Self)) {
        let start = self.i;
        skip(self);
        self.literals.push(start..self.i.min(self.b.len()));
    }

    fn line_comment(&mut self, first: bool) {
        if first {
            self.lines.insert(self.line);
        }
        let start = self.i;
        while self.i < self.b.len() && self.b[self.i] != b'\n' {
            self.i += 1;
        }
        self.spans.push(start..self.i);
    }

    /// Block comments nest in Rust.
    fn block_comment(&mut self) {
        let start = self.i;
        self.block_comment_body();
        self.spans.push(start..self.i.min(self.b.len()));
    }

    fn block_comment_body(&mut self) {
        let mut depth = 0;
        while self.i < self.b.len() {
            if self.at(b"/*") {
                depth += 1;
                self.i += 2;
            } else if self.at(b"*/") {
                depth -= 1;
                self.i += 2;
                if depth == 0 {
                    return;
                }
            } else {
                self.step();
            }
        }
    }

    fn string(&mut self) {
        self.i += 1;
        while self.i < self.b.len() && self.b[self.i] != b'"' {
            if self.b[self.i] == b'\\' {
                self.i += 1;
            }
            if self.i < self.b.len() {
                self.step();
            }
        }
        self.i += 1;
    }

    /// `r"…"`, `r#"…"#`, `br"…"`, `cr#"…"#`. False when the `r`, `b` or `c` starts something else.
    fn raw_string(&mut self) -> bool {
        let starts_word = self.i == 0 || !matches!(self.b[self.i - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_');
        let r = self.i + usize::from(self.b[self.i] != b'r');
        if !starts_word || self.b.get(r) != Some(&b'r') {
            return false;
        }
        let hashes = self.b[r + 1..].iter().take_while(|&&h| h == b'#').count();
        if self.b.get(r + 1 + hashes) != Some(&b'"') {
            return false;
        }
        self.i = r + hashes + 2;
        let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
        while self.i < self.b.len() && !self.at(&close) {
            self.step();
        }
        self.i += close.len();
        true
    }

    /// A character literal (`'x'`, `'\n'`, `'\u{1F600}'`, `'日'`), or a lifetime or label.
    fn char_or_lifetime(&mut self) {
        let start = self.i;
        if self.b.get(self.i + 1) == Some(&b'\\') {
            self.i += 3;
            while self.i < self.b.len() && self.b[self.i] != b'\'' {
                self.i += 1;
            }
            self.i += 1;
            self.literals.push(start..self.i.min(self.b.len()));
            return;
        }
        let width = self.source[self.i + 1..].chars().next().map_or(1, char::len_utf8);
        if self.b.get(self.i + 1 + width) == Some(&b'\'') {
            self.i += width + 2;
            self.literals.push(start..self.i);
        } else {
            self.i += 1;
        }
    }
}

struct FnState {
    name: String,
    line: u32,
    cyclomatic: u32,
    cognitive: u32,
    nesting: u32,
    span: std::ops::Range<usize>,
    /// Scope entries (modules, impls, traits) opened inside this function.
    scope_base: usize,
}

#[derive(Default)]
struct Analyser<'s> {
    source: &'s str,
    lines: Vec<&'s str>,
    /// Lines that start with a real `//` comment, not text inside a string or a block comment.
    comment_lines: HashSet<u32>,
    scope: Vec<String>,
    stack: Vec<FnState>,
    names: crate::lang::Names,
    functions: Vec<MeasuredFunction>,
    overrides: Vec<NativeOverride>,
    /// Names for closures, decided by the expression around them before they are visited.
    closure_names: HashMap<*const ExprClosure, String>,
    /// Inner links of a logical expression, already scored with the outermost one.
    chain_links: HashSet<*const syn::ExprBinary>,
}

impl Analyser<'_> {
    fn qualified(&self, own: &str) -> String {
        let base = self.stack.last().map_or(0, |f| f.scope_base);
        let mut parts: Vec<&str> = self.scope[base..].iter().map(String::as_str).collect();
        parts.push(own);
        let path = parts.join("::");
        match self.stack.last() {
            Some(parent) => format!("{}>{path}", parent.name),
            None => path,
        }
    }

    fn enter(&mut self, own: &str, line: u32, nesting: u32, span: std::ops::Range<usize>) -> String {
        let base = self.qualified(own);
        let name = self.names.add(base, self.stack.last().map(|f| f.name.as_str()));
        self.stack.push(FnState {
            name: name.clone(),
            line,
            cyclomatic: 1,
            cognitive: 0,
            nesting,
            span,
            scope_base: self.scope.len(),
        });
        name
    }

    fn leave(&mut self) {
        let f = self.stack.pop().expect("entered");
        self.functions.push(MeasuredFunction {
            origin: self.names.origin(&f.name),
            name: f.name,
            line: f.line,
            cyclomatic: f.cyclomatic,
            cognitive: f.cognitive,
            span: f.span,
        });
    }

    fn cyclomatic(&mut self) {
        if let Some(f) = self.stack.last_mut() {
            f.cyclomatic += 1;
        }
    }

    /// `nested`: the structure also pays for the nesting around it.
    fn cognitive(&mut self, nested: bool) {
        if let Some(f) = self.stack.last_mut() {
            f.cognitive += 1 + if nested { f.nesting } else { 0 };
        }
    }

    fn nested(&mut self, visit: impl FnOnce(&mut Self)) {
        if let Some(f) = self.stack.last_mut() {
            f.nesting += 1;
        }
        visit(self);
        if let Some(f) = self.stack.last_mut() {
            f.nesting -= 1;
        }
    }

    /// A named function: record overrides on it, then measure its body.
    fn function(&mut self, attrs: &[Attribute], ident: &syn::Ident, body: &Block) {
        let line = line_of(ident.span());
        let span = ident.span().byte_range().start..body.span().byte_range().end;
        let name = self.enter(&ident.to_string(), line, 0, span);
        for attr in attrs {
            if let Some((metric, text)) = attribute_override(attr, self.source) {
                let line = line_of(attr.span());
                self.overrides.push(NativeOverride {
                    line,
                    text,
                    metric,
                    function: Some(name.clone()),
                });
            }
        }
        // Comments directly above the item, or between its attributes and its signature.
        let first = attrs.iter().map(|a| line_of(a.span())).min().unwrap_or(line).min(line);
        let mut above = first.saturating_sub(1);
        while above >= 1 && self.comment_lines.contains(&above) {
            above -= 1;
        }
        for number in ((above + 1)..line).filter(|n| self.comment_lines.contains(n)) {
            let text = self.lines[number as usize - 1].trim();
            for metric in directive_metrics(text) {
                self.overrides.push(NativeOverride {
                    line: number,
                    text: text.to_string(),
                    metric,
                    function: Some(name.clone()),
                });
            }
        }
        self.visit_block(body);
        self.leave();
    }

    fn if_chain(&mut self, e: &syn::ExprIf, else_if: bool) {
        self.cyclomatic();
        self.cognitive(!else_if);
        self.visit_expr(&e.cond);
        self.nested(|a| a.visit_block(&e.then_branch));
        match e.else_branch.as_ref().map(|(_, expr)| &**expr) {
            Some(Expr::If(next)) => self.if_chain(next, true),
            Some(other) => {
                self.cognitive(false);
                self.nested(|a| a.visit_expr(other));
            }
            None => {}
        }
    }
}

/// `self.items.iter().map(|x| …)` → `self.items.iter().map()`: what a closure is passed to, with
/// call arguments left out. Anything else is its source text, shortened.
fn render(expr: &Expr, source: &str) -> String {
    match expr {
        Expr::Path(p) => p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        Expr::MethodCall(m) => format!("{}.{}()", render(&m.receiver, source), m.method),
        Expr::Field(f) => format!(
            "{}.{}",
            render(&f.base, source),
            quote::ToTokens::to_token_stream(&f.member)
        ),
        Expr::Call(c) => format!("{}()", render(&c.func, source)),
        Expr::Reference(r) => render(&r.expr, source),
        Expr::Paren(p) => render(&p.expr, source),
        Expr::Try(t) => render(&t.expr, source),
        Expr::Await(a) => format!("{}.await", render(&a.base, source)),
        other => {
            let text = source.get(other.span().byte_range()).unwrap_or("expr");
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.chars().count() > 40 {
                format!("{}...", text.chars().take(37).collect::<String>())
            } else {
                text
            }
        }
    }
}

/// The last path segment of a type, without generics: `Order<T>` → `Order`.
fn type_name(ty: &Type) -> String {
    match ty {
        Type::Path(p) => p.path.segments.last().map_or("type".into(), |s| s.ident.to_string()),
        Type::Reference(r) => type_name(&r.elem),
        Type::Paren(p) => type_name(&p.elem),
        other => quote::ToTokens::to_token_stream(other).to_string().replace(' ', ""),
    }
}

fn unparen(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => unparen(&p.expr),
        Expr::Group(g) => unparen(&g.expr),
        other => other,
    }
}

fn pat_name(pat: &Pat) -> Option<String> {
    match pat {
        Pat::Ident(i) => Some(i.ident.to_string()),
        Pat::Type(t) => pat_name(&t.pat),
        _ => None,
    }
}

impl<'ast> Visit<'ast> for Analyser<'_> {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.function(&f.attrs, &f.sig.ident, &f.block);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.function(&f.attrs, &f.sig.ident, &f.block);
    }

    fn visit_trait_item_fn(&mut self, f: &'ast syn::TraitItemFn) {
        if let Some(body) = &f.default {
            self.function(&f.attrs, &f.sig.ident, body);
        }
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let owner = match &i.trait_ {
            Some((path, _)) => {
                let trait_name = path.segments.last().map_or("trait".into(), |s| s.ident.to_string());
                format!("<{} as {trait_name}>", type_name(&i.self_ty))
            }
            None => type_name(&i.self_ty),
        };
        self.scope.push(owner);
        visit::visit_item_impl(self, i);
        self.scope.pop();
    }

    fn visit_item_trait(&mut self, t: &'ast syn::ItemTrait) {
        self.scope.push(t.ident.to_string());
        visit::visit_item_trait(self, t);
        self.scope.pop();
    }

    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        // An inner attribute covers the whole module, so it approves nothing.
        for attr in &m.attrs {
            if matches!(attr.style, syn::AttrStyle::Inner(_))
                && let Some((metric, text)) = attribute_override(attr, self.source)
            {
                self.overrides.push(NativeOverride {
                    line: line_of(attr.span()),
                    text,
                    metric,
                    function: None,
                });
            }
        }
        self.scope.push(m.ident.to_string());
        visit::visit_item_mod(self, m);
        self.scope.pop();
    }

    fn visit_expr_closure(&mut self, c: &'ast ExprClosure) {
        let own = self
            .closure_names
            .remove(&(c as *const _))
            .unwrap_or_else(|| "closure".into());
        let nesting = self.stack.last().map_or(0, |f| f.nesting + 1);
        self.enter(&own, line_of(c.inputs_begin.span()), nesting, c.span().byte_range());
        self.visit_expr(&c.body);
        self.leave();
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        let Some(init) = &l.init else {
            return visit::visit_local(self, l);
        };
        if let (Expr::Closure(c), Some(name)) = (&*init.expr, pat_name(&l.pat)) {
            self.closure_names.insert(c as *const _, name);
        }
        match &init.diverge {
            Some((_, diverge)) => {
                self.cyclomatic();
                self.cognitive(true);
                self.visit_pat(&l.pat);
                self.visit_expr(&init.expr);
                self.nested(|a| a.visit_expr(diverge));
            }
            None => visit::visit_local(self, l),
        }
    }

    fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
        let name = format!("{}.{}()", render(&m.receiver, self.source), m.method);
        for arg in &m.args {
            if let Expr::Closure(c) = arg {
                self.closure_names.insert(c as *const _, name.clone());
            }
        }
        visit::visit_expr_method_call(self, m);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        let name = format!("{}()", render(&call.func, self.source));
        for arg in &call.args {
            if let Expr::Closure(c) = arg {
                self.closure_names.insert(c as *const _, name.clone());
            }
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
        self.if_chain(e, false);
    }

    fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
        self.cyclomatic();
        self.cognitive(true);
        self.visit_expr(&m.expr);
        self.nested(|a| m.arms.iter().for_each(|arm| a.visit_arm(arm)));
    }

    fn visit_pat_guard(&mut self, g: &'ast syn::PatGuard) {
        self.cyclomatic();
        visit::visit_pat_guard(self, g);
    }

    fn visit_expr_for_loop(&mut self, f: &'ast syn::ExprForLoop) {
        self.cyclomatic();
        self.cognitive(true);
        self.visit_pat(&f.pat);
        self.visit_expr(&f.expr);
        self.nested(|a| a.visit_block(&f.body));
    }

    fn visit_expr_while(&mut self, w: &'ast syn::ExprWhile) {
        self.cyclomatic();
        self.cognitive(true);
        self.visit_expr(&w.cond);
        self.nested(|a| a.visit_block(&w.body));
    }

    fn visit_expr_loop(&mut self, l: &'ast syn::ExprLoop) {
        self.cognitive(true);
        self.nested(|a| a.visit_block(&l.body));
    }

    fn visit_expr_break(&mut self, b: &'ast syn::ExprBreak) {
        if b.label.is_some() {
            self.cognitive(false);
        }
        visit::visit_expr_break(self, b);
    }

    fn visit_expr_continue(&mut self, c: &'ast syn::ExprContinue) {
        if c.label.is_some() {
            self.cognitive(false);
        }
        visit::visit_expr_continue(self, c);
    }

    fn visit_expr_binary(&mut self, b: &'ast syn::ExprBinary) {
        if is_logical(&b.op) {
            self.cyclomatic();
            // The outermost logical expression scores every run in it at once; its inner links
            // (through parentheses) add nothing more.
            if !self.chain_links.remove(&(b as *const _)) {
                let mut ops = Vec::new();
                logical_ops(b, &mut ops, &mut self.chain_links);
                let runs = 1 + ops.windows(2).filter(|w| w[0] != w[1]).count();
                (0..runs).for_each(|_| self.cognitive(false));
            }
        }
        visit::visit_expr_binary(self, b);
    }
}

fn is_logical(op: &BinOp) -> bool {
    matches!(op, BinOp::And(_) | BinOp::Or(_))
}

/// The logical expression an operand is, looking through parentheses.
fn as_logical(e: &Expr) -> Option<&syn::ExprBinary> {
    match unparen(e) {
        Expr::Binary(inner) if is_logical(&inner.op) => Some(inner),
        _ => None,
    }
}

/// The logical operators of `b` in source order (`true` for `&&`), looking through parentheses:
/// `a && (b || c) && d` gives `&&, ||, &&`, three runs. Records the inner links, which the visitor
/// reaches next, so they are not scored again.
fn logical_ops(b: &syn::ExprBinary, ops: &mut Vec<bool>, links: &mut HashSet<*const syn::ExprBinary>) {
    if let Some(left) = as_logical(&b.left) {
        links.insert(left);
        logical_ops(left, ops, links);
    }
    ops.push(matches!(b.op, BinOp::And(_)));
    if let Some(right) = as_logical(&b.right) {
        links.insert(right);
        logical_ops(right, ops, links);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(source: &str) -> Vec<(String, u32, u32)> {
        let a = analyse_rust(source);
        assert!(a.error.is_none(), "{:?}", a.error);
        a.functions
            .into_iter()
            .map(|f| (f.name, f.cyclomatic, f.cognitive))
            .collect()
    }

    #[test]
    fn names() {
        let src = r#"
            fn free() {}
            mod billing { fn charge() { fn retry() {} let on_error = |e: u8| e; [1].iter().map(|x| x + 1); } }
            struct Order;
            impl Order { fn total(&self) {} }
            impl std::fmt::Display for Order { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) } }
            trait Validate { fn check(&self) {} fn required(&self); }
        "#;
        let mut names: Vec<_> = scores(src).into_iter().map(|(n, ..)| n).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "<Order as Display>::fmt",
                "Order::total",
                "Validate::check",
                "billing::charge",
                "billing::charge>[1].iter().map()",
                "billing::charge>on_error",
                "billing::charge>retry",
                "free"
            ]
        );
    }

    #[test]
    fn cyclomatic_and_cognitive() {
        // if (+1 / +1), nested for (+1 / +2), nested if with && (+2 / +3 and +1), else (+0 / +1)
        let src = "fn f(a: bool, b: bool, v: &[u8]) -> u8 { if a { for x in v { if *x > 1 && b { return 1; } } } else { return 2; } 0 }";
        assert_eq!(scores(src), [("f".into(), 5, 8)]);
        // A match counts once for cyclomatic; each guard counts; arms nest for cognitive.
        let src = "fn g(n: u8) -> u8 { match n { 0 => 1, x if x > 5 => 2, _ => { if n == 3 { 3 } else { 4 } } } }";
        assert_eq!(scores(src), [("g".into(), 4, 4)]);
        // `a && b && c` is one run; `|| d` is a second.
        assert_eq!(
            scores("fn h(a: bool, b: bool, c: bool, d: bool) -> bool { a && b && c || d }"),
            [("h".into(), 4, 2)]
        );
        // `?` is not a decision; `let … else` is.
        assert_eq!(
            scores(
                "fn i(x: Option<u8>) -> Option<u8> { let Some(y) = x else { return None }; Some(y.checked_add(1)?) }"
            ),
            [("i".into(), 2, 1)]
        );
    }

    #[test]
    fn closures_score_separately_one_level_deeper() {
        let src = "fn f(v: Vec<u8>) { if true { v.iter().for_each(|x| if *x > 1 { println!(\"{x}\") }); } }";
        assert_eq!(
            scores(src),
            [("f>v.iter().for_each()".into(), 2, 3), ("f".into(), 2, 1)]
        );
    }

    #[test]
    fn overrides() {
        let src = r#"
#![allow(clippy::cognitive_complexity)]
/// Docs.
#[expect(clippy::cognitive_complexity, reason = "follows the spec")]
fn spec() {}
// autoratchet-disable-next-line complexity -- generated table
#[inline]
fn table() {}
fn plain() {}
"#;
        let a = analyse_rust(src);
        let found: Vec<_> = a
            .overrides
            .iter()
            .map(|o| (o.function.clone(), o.metric, o.line))
            .collect();
        assert_eq!(
            found,
            [
                (None, Metric::Cognitive, 2),
                (Some("spec".into()), Metric::Cognitive, 4),
                (Some("table".into()), Metric::Cyclomatic, 6),
            ]
        );
    }

    fn approved(source: &str) -> Vec<String> {
        analyse_rust(source)
            .overrides
            .into_iter()
            .filter_map(|o| o.function)
            .collect()
    }

    #[test]
    fn only_the_lint_path_approves() {
        let src = r#"
#[allow(dead_code, reason = "clippy::cognitive_complexity is unrelated")]
fn f() {}
#[allow(clippy::cognitive_complexity, dead_code)]
fn g() {}
#[expect(clippy :: cognitive_complexity, reason = "spec order")]
fn h() {}
#[allow(clippy::cognitive_complexity_other)]
fn i() {}
"#;
        assert_eq!(approved(src), ["g", "h"]);
    }

    #[test]
    fn directives_inside_strings_are_not_comments() {
        let src = "#[doc = r#\"\n// autoratchet-disable-next-line complexity -- inside a doc string\n\"#]\nfn f() {}\n\
const S: &str = \"\n// autoratchet-disable-next-line complexity\";\nfn g() {}\n\
const C: char = '\"';\n// autoratchet-disable-next-line complexity -- after a char literal\nfn h() {}\n\
/* outer /* nested */\n// autoratchet-disable-next-line complexity\n*/\nfn k() {}\n\
// autoratchet-disable-next-line complexity -- a real comment\nfn m() {}\n";
        assert_eq!(approved(src), ["h", "m"]);
    }

    #[test]
    fn comment_lexer_survives_lifetimes_and_literals() {
        let src = "fn a<'x>(s: &'x str) -> &'static str { 'outer: loop { break 'outer; } \"\" }\n\
const Q: char = '\\'';\nconst B: u8 = b'\"';\nconst E: char = '\\u{1F600}';\nconst U: char = '日';\n\
const BS: &[u8] = b\"// autoratchet-disable-next-line complexity\";\n\
const RB: &[u8] = br#\"\n// autoratchet-disable-next-line complexity\n\"#;\n\
// autoratchet-disable-next-line complexity -- real\nfn z() {}\n";
        assert_eq!(approved(src), ["z"]);
        assert_eq!(comment_lines(src).len(), 1);
    }

    #[test]
    fn parentheses_do_not_split_a_logical_run() {
        let cognitive = |src: &str| scores(src)[0].2;
        assert_eq!(cognitive("fn p(a: bool, b: bool, c: bool) -> bool { a && b && c }"), 1);
        assert_eq!(
            cognitive("fn p(a: bool, b: bool, c: bool) -> bool { a && (b && c) }"),
            1
        );
        assert_eq!(
            cognitive("fn p(a: bool, b: bool, c: bool) -> bool { (a && b) && c }"),
            1
        );
        assert_eq!(
            cognitive("fn p(a: bool, b: bool, c: bool) -> bool { a && (b || c) }"),
            2
        );
        assert_eq!(
            cognitive("fn p(a: bool, b: bool, c: bool) -> bool { (a || b) && (b || c) }"),
            3
        );
        // Runs follow source order: `&&, ||, &&` is three runs.
        let four = "fn p(a: bool, b: bool, c: bool, d: bool) -> bool";
        assert_eq!(cognitive(&format!("{four} {{ a && (b || c) && d }}")), 3);
        assert_eq!(cognitive(&format!("{four} {{ a || b && c || d }}")), 3);
        assert_eq!(cognitive(&format!("{four} {{ (a && b) && (c && d) }}")), 1);
        // A logical expression inside a call is its own sequence.
        assert_eq!(cognitive(&format!("{four} {{ a && f(b && c) && d }}")), 2);
    }

    #[test]
    fn parse_errors_are_reported() {
        assert!(analyse_rust("fn broken( {").error.is_some());
    }
}
