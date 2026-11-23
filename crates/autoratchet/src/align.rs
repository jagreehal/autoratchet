//! Which code a change kept: two revisions of a file as lexer tokens, lined up. Comments and the
//! whitespace between tokens do not count; literals count exactly as written, and operators
//! split as the language reads them. Whole lines are matched first, so an untouched line keeps
//! its place against a new copy of it, then tokens within what changed, so a statement wrapped
//! or joined across lines still lines up. A finding is the same finding when its context, the
//! code around it up to the brackets or statement it is in, was kept whole.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use similar::{Algorithm, DiffTag, capture_diff_slices};

use crate::lang::{Role, Syntax};
use crate::settings::Settings;

pub(crate) struct Token {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Code,
    Comment,
    Literal,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Gap,
    Word,
    Operator,
    Literal,
    Delimiter,
}

/// A file's syntax, from the parser for its language.
fn syntax(path: &Path, source: &str) -> Syntax {
    let file = path.to_string_lossy();
    if file.ends_with(".py") {
        crate::python::syntax(source)
    } else if file.ends_with(".go") {
        crate::go::syntax(source)
    } else if file.ends_with(".rs") {
        crate::rust::syntax(source)
    } else {
        // As measurement does: a minified bundle can nest thousands of levels deep.
        std::thread::scope(|scope| {
            let parse = || crate::functions::syntax(path, source);
            let thread = std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn_scoped(scope, parse);
            thread.ok().and_then(|t| t.join().ok()).unwrap_or_default()
        })
    }
}

/// What each byte of a file is part of.
fn parts(syntax: &Syntax, source: &str) -> Vec<Part> {
    let mut parts = vec![Part::Code; source.len()];
    for (spans, part) in [(&syntax.literals, Part::Literal), (&syntax.comments, Part::Comment)] {
        for span in spans {
            parts
                .get_mut(span.clone())
                .into_iter()
                .flatten()
                .for_each(|p| *p = part);
        }
    }
    parts
}

fn kind(part: Part, c: char) -> Kind {
    match part {
        Part::Comment => Kind::Gap,
        Part::Literal => Kind::Literal,
        Part::Code if c.is_whitespace() => Kind::Gap,
        Part::Code if c.is_alphanumeric() || c == '_' || c == '$' => Kind::Word,
        Part::Code if "+-*/%=<>!&|^~?:@".contains(c) => Kind::Operator,
        Part::Code => Kind::Delimiter,
    }
}

/// The operators of more than one character a file's language has, longest first: a run of
/// operator characters splits into them, longest match first, as its lexer would.
fn operators(path: &Path) -> &'static [&'static str] {
    let file = path.to_string_lossy();
    if file.ends_with(".py") {
        &[
            "**=", "//=", ">>=", "<<=", "->", ":=", "**", "//", "==", "!=", "<=", ">=", "<<", ">>", "+=", "-=", "*=",
            "/=", "%=", "&=", "|=", "^=", "@=",
        ]
    } else if file.ends_with(".go") {
        &[
            "<<=", ">>=", "&^=", "&&", "||", "<-", "++", "--", "==", "!=", "<=", ">=", ":=", "+=", "-=", "*=", "/=",
            "%=", "&=", "|=", "^=", "&^", "<<", ">>",
        ]
    } else if file.ends_with(".rs") {
        &[
            "<<=", ">>=", "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=", "&=",
            "|=", "^=", "<<", ">>",
        ]
    } else {
        &[
            ">>>=", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=", "&&",
            "||", "??", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "**", "<<", ">>",
        ]
    }
}

/// Split each run of operator characters into the language's operators.
fn split_operators(tokens: Vec<Token>, operators: &[&str]) -> Vec<Token> {
    let mut out = Vec::with_capacity(tokens.len());
    for token in tokens {
        let is_operator = token.text.chars().all(|c| kind(Part::Code, c) == Kind::Operator);
        if !is_operator || token.text.len() < 2 {
            out.push(token);
            continue;
        }
        let mut rest = token.text.as_str();
        let mut at = token.start;
        while let Some(c) = rest.chars().next() {
            let len = operators
                .iter()
                .find(|op| rest.starts_with(**op))
                .map_or(c.len_utf8(), |op| op.len());
            out.push(Token {
                start: at,
                end: at + len,
                line: token.line,
                text: rest[..len].to_string(),
            });
            (rest, at) = (&rest[len..], at + len);
        }
    }
    out
}

/// A file's tokens: words, operators as its language splits them (`==-` is `==` `-`, `++` one,
/// `+ +` two), each delimiter, and each literal whole.
fn tokens(path: &Path, source: &str, syntax: &Syntax) -> Vec<Token> {
    let parts = parts(syntax, source);
    let mut tokens: Vec<Token> = Vec::new();
    let (mut line, mut previous) = (1, Kind::Gap);
    for (at, c) in source.char_indices() {
        let kind = kind(parts[at], c);
        let extends = kind == previous && !matches!(kind, Kind::Gap | Kind::Delimiter);
        match tokens.last_mut() {
            Some(token) if extends => {
                token.text.push(c);
                token.end = at + c.len_utf8();
            }
            _ if kind != Kind::Gap => tokens.push(Token {
                start: at,
                end: at + c.len_utf8(),
                line,
                text: c.to_string(),
            }),
            _ => {}
        }
        previous = kind;
        line += u32::from(c == '\n');
    }
    split_operators(tokens, operators(path))
}

/// The tokens a byte span touches; a span of no bytes, the token it starts.
pub(crate) fn covering(tokens: &[Token], span: &Range<usize>) -> Range<usize> {
    let first = tokens.partition_point(|t| t.end <= span.start);
    let last = tokens.partition_point(|t| t.start < span.end.max(span.start + 1));
    first..last.max(first)
}

/// What a line break outside brackets does to a statement, by language.
#[derive(Clone, Copy)]
enum Breaks {
    /// JavaScript, TypeScript, Rust: nothing; `;` and braces end statements.
    Never,
    /// Python: ends it, unless the line ends in `\`.
    Always,
    /// Go: ends it after an operand, a closing bracket, `++` or `--`, as Go inserts a `;` there.
    AfterOperand,
}

fn breaks(path: &Path) -> Breaks {
    let file = path.to_string_lossy();
    if file.ends_with(".py") {
        Breaks::Always
    } else if file.ends_with(".go") {
        Breaks::AfterOperand
    } else {
        Breaks::Never
    }
}

/// Operators that assign: what is assigned to is not part of the value's expression.
pub(crate) const ASSIGNMENTS: &[&str] = &[
    "=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>=", ">>>=", "**=", "//=", "&&=", "||=", "??=", ":=",
    "@=", "&^=", "=>",
];

/// One revision of a file.
struct Revision {
    source: String,
    tokens: Vec<Token>,
    nodes: Nested<()>,
    scopes: Nested<Role>,
    /// For each token, the tokens around it as far as an expression can go.
    segments: Vec<Range<usize>>,
    /// And as far as a statement can.
    extents: Vec<Range<usize>>,
    hashes: Hashes,
    settings: Settings,
    /// For each scope, as tokens, what it is in.
    placed: Vec<Placed>,
}

struct Placed {
    tokens: Range<usize>,
    enclosing: Enclosing,
}

/// The branches, loops and other statements something is in, by what starts them, `if (a)`,
/// `else`, `case 1:`: all of them, and the nearest.
#[derive(Clone, Copy, Default)]
struct Enclosing {
    all: u64,
    nearest: u64,
}

/// What tells a finding from others across revisions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Identity {
    /// Where its code sits in base, as base's bytes; none for a head finding whose code did not
    /// keep a place there.
    pub place: Option<u64>,
    /// The byte in base of the first token reported, when it at least kept its place.
    pub anchor: Option<u64>,
    /// What it is, as hashes of its tokens and those around it, surest first: `WAYS` of them,
    /// some of which it may lack.
    pub told: Vec<Option<u64>>,
}

/// How far out a finding is told by the code it is in.
// Lookalikes that differ only further out than this are told by what starts the statements
// around them.
const DEEP: usize = 16;

/// How many ways a finding is told: by the code around it, whole or before it; by what it is in and
/// where its variables were set; and by what it is in alone.
const WAYS: usize = 2 * DEEP + 1 + 2 * 2 + 3 * 3;

impl Identity {
    /// How many ways there are to pair findings of two revisions: each way they are told, first
    /// with their place; and last, code that kept its place but was edited. A place alone
    /// counts for less than its code, as other code may have taken it.
    pub const LEVELS: usize = 2 * WAYS + 1;

    pub fn key(&self, level: usize) -> Option<[u64; 2]> {
        if level == Self::LEVELS - 1 {
            return Some([self.anchor?, 0]);
        }
        let place = if level.is_multiple_of(2) { self.place? } else { 0 };
        Some([place, (*self.told.get(level / 2)?)?])
    }
}

/// A statement or function around something, whole, and where in it that is.
#[derive(Clone, Copy)]
pub struct Within {
    /// With what that is in: `if (a)`.
    pub in_place: u64,
    pub alone: u64,
}

/// What tells a function from others of its name across revisions.
pub struct Function {
    tokens: Range<usize>,
    /// The token of base it starts with, for one of head's when that was kept.
    pub place: Option<usize>,
    pub code: u64,
    /// The statements around it, inner to outer.
    pub within: Vec<Within>,
    /// What it is in, surest first: the call and all that is around that; the call and the
    /// nearest; anything.
    pub around: [u64; Self::AROUND],
}

impl Function {
    pub const AROUND: usize = 3;

    pub fn size(&self) -> usize {
        self.tokens.len()
    }
}

/// How many tokens either side of a finding its statement is told by.
const NEAR: usize = 100;

impl Revision {
    fn read(path: &Path) -> Self {
        let source = String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned();
        let mut syntax = syntax(path, &source);
        let tokens = tokens(path, &source, &syntax);
        let extents = segments(&tokens, breaks(path), true);
        if syntax.scopes.is_empty() {
            syntax.scopes = brace_scopes(&tokens, &extents);
        }
        let mut scopes = nested(syntax.scopes);
        bare_blocks(&mut scopes, &source);
        let mut revision = Revision {
            placed: Vec::with_capacity(scopes.len()),
            nodes: nested(syntax.nodes.into_iter().map(|node| (node, ())).collect()),
            segments: segments(&tokens, breaks(path), false),
            settings: Settings::of(&tokens, &scopes, &extents),
            hashes: Hashes::of(&tokens),
            scopes,
            extents,
            tokens,
            source,
        };
        for i in 0..revision.scopes.len() {
            let (node, _, outer) = &revision.scopes[i];
            let tokens = covering(&revision.tokens, node);
            let enclosing = outer.and_then(|outer| revision.enclosing_from(&tokens, outer));
            let enclosing = enclosing.unwrap_or_default();
            revision.placed.push(Placed { tokens, enclosing });
        }
        revision
    }

    /// A finding's context: the expression its span is in, as tokens. The smallest syntax node
    /// around it of three tokens or more (`x == 1` for its `==`, not the `&&` it is part of; a
    /// lone `any` says too little), within its surroundings; those, where there is none.
    fn context(&self, span: &Range<usize>) -> Option<Range<usize>> {
        let inner = covering(&self.tokens, span);
        let last = inner.end.checked_sub(1).filter(|_| !inner.is_empty())?;
        let surroundings = self.segments[inner.start].start..self.segments[last].end;
        around(&self.nodes, span)
            .map(|(node, ())| covering(&self.tokens, node))
            .take_while(|c| surroundings.start <= c.start && c.end <= surroundings.end)
            .find(|c| c.len() >= 3)
            .or(Some(surroundings))
    }

    /// A finding's statement, holding its context: the smallest around it, without its blocks.
    fn statement(&self, span: &Range<usize>, context: &Range<usize>) -> Range<usize> {
        let extent = self.extents[context.start].start..self.extents[context.end - 1].end;
        let node = around(&self.scopes, span)
            .find(|(_, scope)| *scope != Role::Block)
            .map(|(node, _)| covering(&self.tokens, node));
        let statement = node.map_or(extent.clone(), |n| n.start.max(extent.start)..n.end.min(extent.end));
        statement.start.min(context.start)..statement.end.max(context.end)
    }

    /// The statements and functions around a finding's own statement, inner to outer.
    fn around(&self, span: &Range<usize>, statement: &Range<usize>) -> impl Iterator<Item = usize> {
        // Its own has the `;` that ends it.
        let semicolon = self.tokens.get(statement.end).is_some_and(|t| t.text == ";");
        let own = statement.start..statement.end + usize::from(semicolon);
        let larger = move |i: &usize| {
            let outer = &self.placed[*i].tokens;
            outer.start < own.start || outer.end > own.end
        };
        // A callback says too little without its call.
        let tells = |i: &usize| match self.scopes[*i].1 {
            Role::Function => !self.is_argument(&self.placed[*i].tokens),
            role => role != Role::Block,
        };
        let scopes: Vec<usize> = indices_around(&self.scopes, span)
            .filter(tells)
            .filter(larger)
            .take(DEEP)
            .collect();
        scopes.into_iter()
    }

    /// Those scopes, whole, in place and alone, and where in each something at token `at` is.
    fn within(&self, span: &Range<usize>, statement: &Range<usize>, at: usize) -> Vec<Within> {
        let told = self.around(span, statement).map(|i| {
            let Placed { tokens, enclosing } = &self.placed[i];
            let alone = mix(&[self.hashes.of_run(tokens), (at - tokens.start) as u64]);
            let place = if self.scopes[i].1 == Role::Function {
                0
            } else {
                enclosing.all
            };
            Within {
                in_place: mix(&[alone, place]),
                alone,
            }
        });
        told.collect()
    }

    fn is_argument(&self, function: &Range<usize>) -> bool {
        let before = function.start.checked_sub(1).map(|i| self.tokens[i].text.as_str());
        matches!(before, Some("(" | ","))
    }

    /// What a statement is in, as far as its function, and for a function what that is in.
    fn enclosing(&self, span: &Range<usize>, statement: &Range<usize>, of_function: bool) -> Enclosing {
        let mut inner = statement.clone();
        for i in indices_around(&self.scopes, span) {
            let outer = &self.placed[i].tokens;
            if of_function && outer == statement {
                continue;
            }
            if let Some(found) = self.enclosing_from(&inner, i) {
                return found;
            }
            inner = inner.start.min(outer.start)..inner.end.max(outer.end);
        }
        Enclosing::default()
    }

    /// The same for a run of tokens in a scope, unless the scope is in the run.
    fn enclosing_from(&self, inner: &Range<usize>, scope: usize) -> Option<Enclosing> {
        let Placed {
            tokens: outer,
            enclosing,
        } = &self.placed[scope];
        match self.scopes[scope].1 {
            // A callback is in the call it is given to; any other function is where this ends.
            Role::Function if !self.is_argument(outer) => Some(Enclosing::default()),
            Role::Statement if outer.start < inner.start => {
                let header = outer.start..self.extents[outer.start].end.min(inner.start);
                // Which of its blocks: the one its header is for has none of these before it.
                let before = inner.start - 1..inner.start;
                let part = matches!(self.tokens[before.start].text.as_str(), "else" | "finally");
                let part = if part { self.hashes.of_run(&before) } else { 0 };
                let nearest = mix(&[self.hashes.of_run(&header), part]);
                Some(Enclosing {
                    nearest,
                    all: mix(&[nearest, enclosing.all]),
                })
            }
            _ => (outer.start <= inner.start && inner.end <= outer.end).then_some(*enclosing),
        }
    }

    fn identity(&self, span: &Range<usize>, place: Option<Range<usize>>, anchor: Option<usize>) -> Option<Identity> {
        let place = place.map(|place| mix(&[place.start as u64, place.end as u64]));
        let anchor = anchor.map(|at| at as u64);
        let told = match self.context(span) {
            Some(context) => self.told(span, &context),
            // A finding on a comment: what it says.
            None => vec![Some(hash(self.source.get(span.clone())?.trim()))],
        };
        Some(Identity { place, anchor, told })
    }

    /// What a finding is, the surest way first.
    fn told(&self, span: &Range<usize>, context: &Range<usize>) -> Vec<Option<u64>> {
        let statement = self.statement(span, context);
        let hash = |run: Range<usize>| self.hashes.of_run(&run);
        // As much of its statement as is near: an edit far off in a large one does not count.
        let start = statement.start.max(context.start.saturating_sub(NEAR));
        let whole = hash(start..statement.end.min(context.end + NEAR));
        // As far as its code: `log(x == 1` of `log(x == 1, y)`.
        let frame = hash(start..context.end);
        let code = hash(context.clone());
        // Where its variables were last set: `x = value.first`.
        let settings = self.settings.seen_by(&self.tokens, context, statement.start);
        let settings: Vec<u64> = settings.into_iter().map(hash).collect();
        let enclosing = self.enclosing(span, &statement, false);
        let Enclosing {
            all: enclosing,
            nearest,
        } = enclosing;
        let origin = mix(&settings);

        let within = self.within(span, &statement, context.start);
        let whole_at = |depth: usize| within.get(DEEP - 1 - depth);
        let mut told: Vec<Option<u64>> = (0..DEEP).map(|depth| Some(whole_at(depth)?.in_place)).collect();
        // An edit after it leaves what comes before, in what it is in.
        let part = self
            .around(span, &statement)
            .next()
            .filter(|&i| self.scopes[i].1 != Role::Function);
        told.push(part.map(|i| mix(&[enclosing, hash(self.placed[i].tokens.start..context.end)])));
        told.extend([whole, frame].map(|text| Some(mix(&[enclosing, origin, text]))));
        told.extend([whole, frame].map(|text| (!settings.is_empty()).then(|| mix(&[origin, text]))));
        let texts = |around| [whole, frame, code].map(|text| Some(mix(&[around, text])));
        told.extend(texts(enclosing));
        told.extend((0..DEEP).map(|depth| Some(whole_at(depth)?.alone)));
        told.extend([nearest, 0].into_iter().flat_map(texts));
        told
    }

    /// What tells a function from others of its name, which number them in order.
    fn function(&self, span: &Range<usize>, place: impl Fn(usize) -> Option<usize>) -> Function {
        let tokens = covering(&self.tokens, span);
        let hash = |run: Range<usize>| self.hashes.of_run(&run);
        let enclosing = self.enclosing(span, &tokens, true);
        // What stands before it in its statement: `run(a, ` of `run(a, () => 1)`.
        let call = self
            .extents
            .get(tokens.start)
            .map_or(0, |extent| hash(extent.start..tokens.start));
        Function {
            place: place(tokens.start),
            within: self.within(span, &tokens, tokens.start),
            code: hash(tokens.clone()),
            around: [mix(&[call, enclosing.all]), mix(&[call, enclosing.nearest]), 0],
            tokens,
        }
    }

    /// The bytes a run of tokens spans.
    fn bytes(&self, run: &Range<usize>) -> Range<usize> {
        self.tokens[run.start].start..self.tokens[run.end - 1].end
    }
}

/// Nodes in order, outer before inner, each with the index of the one around it.
pub(crate) type Nested<T> = Vec<(Range<usize>, T, Option<usize>)>;

/// The nodes around a span, inner to outer. The last node to start by it is in every one.
fn indices_around<'n, T>(nodes: &'n Nested<T>, span: &'n Range<usize>) -> impl Iterator<Item = usize> + 'n {
    let last = nodes
        .partition_point(|(node, ..)| node.start <= span.start)
        .checked_sub(1);
    let outwards = std::iter::successors(last, |&i| nodes[i].2);
    outwards.filter(|&i| nodes[i].0.start <= span.start && span.end <= nodes[i].0.end)
}

pub(crate) fn around<'n, T: Copy>(
    nodes: &'n Nested<T>,
    span: &'n Range<usize>,
) -> impl Iterator<Item = (&'n Range<usize>, T)> {
    indices_around(nodes, span).map(|i| (&nodes[i].0, nodes[i].1))
}

fn nested<T>(mut nodes: Vec<(Range<usize>, T)>) -> Nested<T> {
    nodes.retain(|(node, _)| !node.is_empty());
    // Of two alike, the one found first is the outer one.
    nodes.sort_by_key(|(node, _)| (node.start, std::cmp::Reverse(node.end)));
    let mut open: Vec<usize> = Vec::new();
    let mut out: Nested<T> = Vec::with_capacity(nodes.len());
    for (node, of) in nodes {
        while open.pop_if(|i| out[*i].0.end < node.end).is_some() {}
        out.push((node, of, open.last().copied()));
        open.push(out.len() - 1);
    }
    out
}

/// A block among statements, part of none, is a statement.
fn bare_blocks(scopes: &mut Nested<Role>, source: &str) {
    for i in 0..scopes.len() {
        let in_block = scopes[i].2.is_none_or(|outer| scopes[outer].1 == Role::Block);
        if scopes[i].1 == Role::Block && in_block && source[scopes[i].0.start..].starts_with('{') {
            scopes[i].1 = Role::Statement;
        }
    }
}

/// Scopes without syntax, for Rust: each block, the statement it is in, from what stands before
/// its `{`, and for `if a { } else { }` and the like the statement they make together.
fn brace_scopes(tokens: &[Token], extents: &[Range<usize>]) -> Vec<(Range<usize>, Role)> {
    let mut found = Vec::new();
    // Where the statement starts whose last block ended just before a token.
    let mut going_on: HashMap<usize, usize> = HashMap::new();
    for (open, close) in braces(tokens) {
        let before = open.checked_sub(1).filter(|&b| !is_one_of(tokens, b, &[";", "{", "}"]));
        let header = before.map_or(open, |b| extents[b].start);
        let next = close + 1;
        let goes_on = is_one_of(tokens, next, &["else", ".", "?", "=", ")", "]", ";"]);
        let start = going_on.remove(&header).unwrap_or(header);
        if goes_on {
            going_on.insert(next, start);
        }
        let bytes = |from: usize, to: usize| tokens[from].start..tokens[to - 1].end;
        let function = (header..open).any(|i| tokens[i].text == "fn");
        let role = if function { Role::Function } else { Role::Statement };
        let end = if goes_on { extents[next].end } else { next };
        found.push((bytes(start, end), Role::Statement));
        found.push((bytes(header, next), role));
        found.push((bytes(open, next), Role::Block));
    }
    found
}

fn is_one_of(tokens: &[Token], at: usize, texts: &[&str]) -> bool {
    tokens.get(at).is_some_and(|t| texts.contains(&t.text.as_str()))
}

/// Each `{` and its `}`, by which closes first.
fn braces(tokens: &[Token]) -> Vec<(usize, usize)> {
    let mut open = Vec::new();
    let mut pairs = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match token.text.as_str() {
            "{" => open.push(i),
            "}" => pairs.extend(open.pop().map(|start| (start, i))),
            _ => {}
        }
    }
    pairs
}

/// A hash of some text.
fn hash(text: &str) -> u64 {
    SEED.hash_one(text)
}

fn mix(parts: &[u64]) -> u64 {
    if parts.is_empty() { 0 } else { SEED.hash_one(parts) }
}

/// Not the same from run to run, so no change can be written to collide.
static SEED: LazyLock<RandomState> = LazyLock::new(RandomState::new);

/// Hashes of the tokens up to each, so that any run's hash takes no time: polynomials over a
/// prime field, at a random point.
struct Hashes {
    up_to: Vec<u64>,
    powers: Vec<u64>,
}

impl Hashes {
    const PRIME: u64 = (1 << 61) - 1;

    fn times(a: u64, b: u64) -> u64 {
        (u128::from(a) * u128::from(b) % u128::from(Self::PRIME)) as u64
    }

    fn of(tokens: &[Token]) -> Self {
        let point = hash("point") % Self::PRIME;
        let (mut up_to, mut powers) = (vec![0], vec![1]);
        let (mut sum, mut power) = (0, 1);
        for token in tokens {
            sum = (Self::times(sum, point) + hash(&token.text) % Self::PRIME) % Self::PRIME;
            power = Self::times(power, point);
            up_to.push(sum);
            powers.push(power);
        }
        Hashes { up_to, powers }
    }

    fn of_run(&self, run: &Range<usize>) -> u64 {
        let before = Self::times(self.up_to[run.start], self.powers[run.len()]);
        let sum = (self.up_to[run.end] + Self::PRIME - before) % Self::PRIME;
        mix(&[sum, run.len() as u64])
    }
}

/// For each token, the tokens around it as far as an expression can go without its syntax: up
/// to the brackets around it, a `,` or `;`, an assignment, a block's brace, or the end of a
/// statement, so a call's other arguments and what a value is assigned to are not part of it. Or, for
/// `statements`, as far as a statement can: up to a `;`, a brace, or its end.
fn segments(tokens: &[Token], breaks: Breaks, statements: bool) -> Vec<Range<usize>> {
    let mut segments = Segments::default();
    segments.fresh(0);
    let mut of: Vec<usize> = Vec::with_capacity(tokens.len());
    for (i, token) in tokens.iter().enumerate() {
        let text = token.text.as_str();
        if i > 0 && line_ends(&tokens[i - 1], token, segments.inside(), breaks) {
            segments.fresh(i);
        }
        if matches!(text, ")" | "]" | "}") {
            segments.close(i);
        }
        // A brace or separator is on its own, between what it separates.
        let separates = if statements { text == ";" } else { separates(text) };
        let alone = separates || matches!(text, "{" | "}");
        of.push(segments.add(i, alone));
        if matches!(text, "(" | "[" | "{") {
            segments.open(text, i, !statements);
        }
    }
    of.into_iter().map(|segment| segments.spans[segment].clone()).collect()
}

/// Runs of tokens being made out, and the brackets open around the current one.
#[derive(Default)]
struct Segments<'t> {
    spans: Vec<Range<usize>>,
    current: usize,
    brackets: Vec<(&'t str, usize)>,
}

impl<'t> Segments<'t> {
    /// Start a new run at token `at`.
    fn fresh(&mut self, at: usize) {
        self.spans.push(at..at);
        self.current = self.spans.len() - 1;
    }

    fn inside(&self) -> Option<&'t str> {
        self.brackets.last().map(|(open, _)| *open)
    }

    /// Put token `at` in the current run, or `alone` in one of its own.
    fn add(&mut self, at: usize, alone: bool) -> usize {
        if alone {
            self.fresh(at);
        }
        let own = self.current;
        self.spans[own] = self.spans[own].start.min(at)..at + 1;
        if alone {
            self.fresh(at + 1);
        }
        own
    }

    /// Go into a bracket at token `at`, into a run of its own when it `divides`.
    fn open(&mut self, bracket: &'t str, at: usize, divides: bool) {
        self.brackets.push((bracket, self.current));
        if divides {
            self.fresh(at + 1);
        }
    }

    /// Back out of a bracket, to the run it is in.
    fn close(&mut self, at: usize) {
        match self.brackets.pop() {
            Some((_, outer)) => self.current = outer,
            None => self.fresh(at),
        }
    }
}

/// A `,` ends an argument or element, a `;` a statement, and an assignment its target.
fn separates(text: &str) -> bool {
    matches!(text, "," | ";") || ASSIGNMENTS.contains(&text)
}

/// Whether a line break between two tokens ends the statement, as the language reads it.
fn line_ends(before: &Token, after: &Token, inside: Option<&str>, breaks: Breaks) -> bool {
    if before.line == after.line || matches!(inside, Some("(" | "[")) {
        return false;
    }
    match breaks {
        Breaks::Never => false,
        Breaks::Always => before.text != "\\",
        Breaks::AfterOperand => {
            let operand = before
                .text
                .starts_with(|c: char| c.is_alphanumeric() || "_\"'`".contains(c));
            operand || matches!(before.text.as_str(), ")" | "]" | "}" | "++" | "--")
        }
    }
}

/// The tokens of each line that has any, as index ranges.
fn lines(tokens: &[Token]) -> Vec<Range<usize>> {
    let mut lines: Vec<Range<usize>> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match lines.last_mut() {
            Some(line) if tokens[line.start].line == token.line => line.end = i + 1,
            _ => lines.push(i..i + 1),
        }
    }
    lines
}

/// Head token → base token, for each token the change kept.
fn matches(base: &[Token], head: &[Token]) -> Vec<Option<usize>> {
    let (base_lines, head_lines) = (lines(base), lines(head));
    let key = |tokens: &[Token], line: &Range<usize>| {
        let texts: Vec<&str> = tokens[line.clone()].iter().map(|t| t.text.as_str()).collect();
        texts.join("\0")
    };
    let base_keys: Vec<String> = base_lines.iter().map(|l| key(base, l)).collect();
    let head_keys: Vec<String> = head_lines.iter().map(|l| key(head, l)).collect();
    let base_keys: Vec<&str> = base_keys.iter().map(String::as_str).collect();
    let head_keys: Vec<&str> = head_keys.iter().map(String::as_str).collect();
    let mut map = vec![None; head.len()];
    let end = (base_lines.len(), head_lines.len());
    let (mut old, mut new) = (0, 0);
    for (b, h) in shared(&base_keys, &head_keys).into_iter().chain([end]) {
        // The lines before a kept one changed: line up what they hold.
        within(
            &mut map,
            base,
            head,
            tokens_of(&base_lines, old..b),
            tokens_of(&head_lines, new..h),
        );
        if (b, h) != end {
            for (b, h) in base_lines[b].clone().zip(head_lines[h].clone()) {
                map[h] = Some(b);
            }
        }
        (old, new) = (b + 1, h + 1);
    }
    map
}

/// The tokens of a run of lines.
fn tokens_of(lines: &[Range<usize>], run: Range<usize>) -> Range<usize> {
    if run.is_empty() {
        return 0..0;
    }
    lines[run.start].start..lines[run.end - 1].end
}

/// The largest changed region lined up token by token, as base's tokens times head's.
// Code in a larger rewritten region counts as new, so lining up stays fast.
const LARGEST_REGION: usize = 400_000_000;

/// Line up the tokens of a changed region one by one.
fn within(map: &mut [Option<usize>], base: &[Token], head: &[Token], old: Range<usize>, new: Range<usize>) {
    fn texts<'t>(tokens: &'t [Token], range: &Range<usize>) -> Vec<&'t str> {
        tokens[range.clone()].iter().map(|t| t.text.as_str()).collect()
    }
    if old.len().saturating_mul(new.len()) > LARGEST_REGION {
        return;
    }
    for (b, h) in kept_tokens(&texts(base, &old), &texts(head, &new)) {
        map[new.start + h] = Some(old.start + b);
    }
}

/// The longest sequence two share, as index pairs. What only one side has cannot be in it, and
/// is left out before the diff, whose cost is in what differs.
fn shared(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    fn also_in<'t>(items: &[&'t str], other: &[&str]) -> (Vec<usize>, Vec<&'t str>) {
        let other: HashSet<&str> = other.iter().copied().collect();
        let both = items.iter().enumerate().filter(|(_, item)| other.contains(*item));
        both.map(|(i, item)| (i, *item)).unzip()
    }
    let ((old_at, old), (new_at, new)) = (also_in(old, new), also_in(new, old));
    let ops = capture_diff_slices(Algorithm::Myers, &old, &new);
    let equal = ops.into_iter().filter(|op| op.tag() == DiffTag::Equal);
    let pairs = equal.flat_map(|op| op.old_range().zip(op.new_range()));
    pairs.map(|(o, n)| (old_at[o], new_at[n])).collect()
}

/// The tokens two sequences share: of the alignments a diff finds forwards and backwards, which
/// keep as many tokens, the one that keeps them in fewer runs, so code stays together
/// (`log(x == 1)` whole, not `log(x ==` of one call and `1)` of the next).
fn kept_tokens(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let breaks = |pairs: &[(usize, usize)]| {
        let apart = |pair: &&[(usize, usize)]| pair[1].0.abs_diff(pair[0].0) != 1 || pair[1].1.abs_diff(pair[0].1) != 1;
        pairs.windows(2).filter(apart).count()
    };
    let forward = shared(old, new);
    if breaks(&forward) == 0 {
        return forward;
    }
    let old_back: Vec<&str> = old.iter().rev().copied().collect();
    let new_back: Vec<&str> = new.iter().rev().copied().collect();
    let backward = shared(&old_back, &new_back);
    if breaks(&backward) < breaks(&forward) {
        let turned = backward
            .into_iter()
            .map(|(o, n)| (old.len() - 1 - o, new.len() - 1 - n));
        turned.collect()
    } else {
        forward
    }
}

/// Two revisions of a file, lined up.
pub struct Alignment {
    base: Revision,
    head: Revision,
    /// Head token → base token, for each the change kept.
    map: Vec<Option<usize>>,
}

impl Alignment {
    pub fn new(base: &Path, head: &Path) -> Self {
        let (base, head) = (Revision::read(base), Revision::read(head));
        let map = matches(&base.tokens, &head.tokens);
        Alignment { base, head, map }
    }

    pub fn base_identity(&self, span: &Range<usize>) -> Option<Identity> {
        let first = covering(&self.base.tokens, span).start;
        let anchor = self.base.tokens.get(first).map(|token| token.start);
        self.base.identity(span, self.base_place(span), anchor)
    }

    pub fn head_identity(&self, span: &Range<usize>) -> Option<Identity> {
        let first = covering(&self.head.tokens, span).start;
        let kept = self.map.get(first).copied().flatten();
        let anchor = kept.map(|token| self.base.tokens[token].start);
        self.head.identity(span, self.head_place(span), anchor)
    }

    /// A base finding's place: its context, as base's bytes.
    fn base_place(&self, span: &Range<usize>) -> Option<Range<usize>> {
        self.base.context(span).map(|context| self.base.bytes(&context))
    }

    /// A head finding's place, when the change kept its context whole and in order: the base
    /// bytes that context was kept from, which a base finding of the same code has as its place.
    fn head_place(&self, span: &Range<usize>) -> Option<Range<usize>> {
        let context = self.head.context(span)?;
        let kept: Vec<usize> = context.map(|i| self.map[i]).collect::<Option<_>>()?;
        let in_order = kept.windows(2).all(|pair| pair[1] == pair[0] + 1);
        let (first, last) = (*kept.first()?, *kept.last()?);
        in_order.then(|| self.base.bytes(&(first..last + 1)))
    }

    pub fn base_function(&self, span: &Range<usize>) -> Function {
        self.base.function(span, Some)
    }

    pub fn head_function(&self, span: &Range<usize>) -> Function {
        self.head.function(span, |token| self.map.get(token).copied().flatten())
    }

    /// How many tokens of base's function, in order, head's kept.
    pub fn kept(&self, base: &Function, head: &Function) -> usize {
        let kept = self.map[head.tokens.clone()].iter().flatten();
        kept.filter(|&token| base.tokens.contains(token)).count()
    }

    /// How many tokens the two share, wherever they are.
    pub fn shared(&self, base: &Function, head: &Function) -> usize {
        let mut count: HashMap<&str, usize> = HashMap::new();
        for token in &self.base.tokens[base.tokens.clone()] {
            *count.entry(&token.text).or_default() += 1;
        }
        let shared = self.head.tokens[head.tokens.clone()].iter().filter(|token| {
            let left = count.get_mut(token.text.as_str()).filter(|n| **n > 0);
            left.map(|n| *n -= 1).is_some()
        });
        shared.count()
    }

    /// The base line of the code on head's `line`, when the change kept its first token.
    pub fn base_line(&self, line: u32) -> Option<u32> {
        let at = self.head.tokens.partition_point(|t| t.line < line);
        self.head.tokens.get(at).filter(|t| t.line == line)?;
        self.map.get(at).copied().flatten().map(|b| self.base.tokens[b].line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The base line head's `line` was kept from, by the first token on it.
    fn kept(base: &str, head: &str) -> impl Fn(u32) -> Option<u32> {
        let temp = tempfile::tempdir().expect("temp dir");
        let dir = temp.path();
        std::fs::write(dir.join("base.ts"), base).expect("write");
        std::fs::write(dir.join("head.ts"), head).expect("write");
        let alignment = Alignment::new(&dir.join("base.ts"), &dir.join("head.ts"));
        move |line| alignment.base_line(line)
    }

    #[test]
    fn comments_and_spacing_between_tokens_do_not_count() {
        let base = "function f(x) {\n  if (x == 1) return;\n  if (x==3) return;\n}\n";
        let head = concat!(
            "function f(x) {\n",
            "  if (x == 1) /* oxlint-disable-line eqeqeq -- old */ return;\n",
            "  if (x == 2) return;\n",
            "  if (x /* note */ ==3) return; // still loose: see #12\n",
            "}\n",
        );
        let kept = kept(base, head);
        assert_eq!((kept(2), kept(3), kept(4)), (Some(2), None, Some(3)));
    }

    #[test]
    fn tokens_and_literals_count_exactly() {
        // Spaces inside a string are its contents.
        let strings = kept(
            "function f(s) {\n  if (s == \"a  b\") return;\n}\n",
            "function f(s) {\n  if (s == \"a b\") return;\n  if (s == \"a  b\") return;\n}\n",
        );
        assert_eq!((strings(2), strings(3)), (None, Some(2)));
        // `++y` is one operator, `+ +y` two.
        let operators = kept(
            "function f(x, y) {\n  log(x == + +y);\n}\n",
            "function f(x, y) {\n  log(x == ++y);\n  log(x == + +y);\n}\n",
        );
        assert_eq!((operators(2), operators(3)), (None, Some(2)));
        // Words stay apart: `return x` is not `returnx`.
        let words = kept(
            "function f(x) {\n  return x;\n}\n",
            "function f(x) {\n  return  x;\n  returnx;\n}\n",
        );
        assert_eq!((words(2), words(3)), (Some(2), None));
    }

    /// Whether head's `==` at `head_at` is the same finding as base's at `base_at`.
    fn same(base: &str, base_at: usize, head: &str, head_at: usize) -> bool {
        let temp = tempfile::tempdir().expect("temp dir");
        let dir = temp.path();
        std::fs::write(dir.join("base.ts"), base).expect("write");
        std::fs::write(dir.join("head.ts"), head).expect("write");
        let alignment = Alignment::new(&dir.join("base.ts"), &dir.join("head.ts"));
        let base_place = alignment.base_place(&(base_at..base_at + 2));
        base_place.is_some() && alignment.head_place(&(head_at..head_at + 2)) == base_place
    }

    #[test]
    fn a_finding_is_its_code_in_context() {
        // `==-` is `==` `-`, as `== -` is.
        let base = "function f(x) {\n  if (x==-1) return;\n}\n";
        let head = "function f(x) {\n  if (x == -1) return;\n  if (x == 2) return;\n}\n";
        assert!(same(base, base.find("==").unwrap(), head, head.find("==").unwrap()));
        assert!(!same(base, base.find("==").unwrap(), head, head.rfind("==").unwrap()));
        // A new call before an old one whose `);` moved down: each keeps its own place.
        let base = "function f(x) {\n  log(x == 1);\n}\n";
        let head = "function f(x) {\n  log(x == 2);\n  log(x == 1\n  );\n}\n";
        assert!(!same(base, base.find("==").unwrap(), head, head.find("==").unwrap()));
        assert!(same(base, base.find("==").unwrap(), head, head.rfind("==").unwrap()));
        // Nor is what it is assigned to, however that is named or wrapped.
        let base = "function f(x) {\n  const old = x == 1;\n}\n";
        for head in [
            "function f(x) {\n  const old =\n    x == 1;\n  const other = x == 2;\n}\n",
            "function f(x) {\n  const renamed = x == 1;\n  const other = x == 2;\n}\n",
        ] {
            assert!(
                same(base, base.find("==").unwrap(), head, head.find("==").unwrap()),
                "{head}"
            );
            assert!(
                !same(base, base.find("==").unwrap(), head, head.rfind("==").unwrap()),
                "{head}"
            );
        }
        // Nor is the other side of an `&&`; the comparison's own operands are.
        let base = "function f(x) {\n  log(x == 1 && x > 2);\n}\n";
        let head = "function f(x) {\n  log(x == 1 && x > 3);\n  log(x == 2);\n}\n";
        assert!(same(base, base.find("==").unwrap(), head, head.find("==").unwrap()));
        assert!(!same(base, base.find("==").unwrap(), head, head.rfind("==").unwrap()));
        let edited = "function f(x) {\n  log(x == 4 && x > 2);\n}\n";
        assert!(!same(
            base,
            base.find("==").unwrap(),
            edited,
            edited.find("==").unwrap()
        ));
        // Another argument of the same call is not part of the comparison.
        let base = "function f(x) {\n  log(x == 1, \"before\");\n}\n";
        let head = "function f(x) {\n  log(x == 1, \"after\");\n  log(x == 2);\n}\n";
        assert!(same(base, base.find("==").unwrap(), head, head.find("==").unwrap()));
        assert!(!same(base, base.find("==").unwrap(), head, head.rfind("==").unwrap()));
    }

    #[test]
    fn a_statement_wrapped_onto_two_lines_is_kept() {
        let wrapped = kept(
            "function f(x) {\n  if (x == 1) return true;\n  return false;\n}\n",
            "function f(x) {\n  if (x == 1)\n    return true;\n  if (x == 2) return true;\n  return false;\n}\n",
        );
        assert_eq!(
            (wrapped(2), wrapped(3), wrapped(4), wrapped(5)),
            (Some(2), Some(2), None, Some(3))
        );
    }
}
