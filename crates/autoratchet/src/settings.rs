//! Where variables were set: what `x` is in `log(x == 1)`, by the statements that led to it.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;

use crate::align::{ASSIGNMENTS, Nested, Token, around, covering};
use crate::lang::Role;

/// A statement that sets a variable, and the code it is set for, as tokens.
struct Setting {
    statement: Range<usize>,
    /// Its operator: what it is set to comes after.
    at: usize,
    seen: Range<usize>,
}

/// A file's settings, by the variable or member they set, in order.
pub(crate) struct Settings(HashMap<String, Vec<Setting>>);

/// How far before its operator an assignment's target may start, in tokens; how much of some code
/// is looked through for names, and how many earlier settings of a name for one in sight.
const FAR: usize = 100;

/// How many settings lead to a finding: those of its variables, then of theirs.
const MOST: usize = 8;

impl Settings {
    pub fn of(tokens: &[Token], scopes: &Nested<Role>, extents: &[Range<usize>]) -> Self {
        let mut settings: HashMap<String, Vec<Setting>> = HashMap::new();
        let assigns = |t: &Token| t.text != "=>" && ASSIGNMENTS.contains(&t.text.as_str());
        for at in (0..tokens.len()).filter(|&i| assigns(&tokens[i])) {
            let span = tokens[at].start..tokens[at].end;
            let mut scopes = around(scopes, &span);
            let nearest = scopes.next();
            // In a block, or a file, with no syntax of its own: what can be one statement.
            let (statement, outer) = match nearest {
                Some((node, role)) if role != Role::Block => {
                    (covering(tokens, node), scopes.next().map(|(node, _)| node))
                }
                _ => (extents[at].clone(), nearest.map(|(node, _)| node)),
            };
            let seen = outer.map_or(0..tokens.len(), |node| covering(tokens, node));
            for name in targets(tokens, &statement, at) {
                let (statement, seen) = (statement.clone(), seen.clone());
                settings.entry(name).or_default().push(Setting { statement, at, seen });
            }
        }
        // One inside another's value ends first.
        settings
            .values_mut()
            .for_each(|all| all.sort_by_key(|s| s.statement.end));
        Settings(settings)
    }

    /// The statements that last set the variables of some code before its own statement, and the
    /// variables of those.
    pub fn seen_by(&self, tokens: &[Token], code: &Range<usize>, before: usize) -> Vec<Range<usize>> {
        let mut found: Vec<Range<usize>> = Vec::new();
        let mut look = VecDeque::from([(code.clone(), before)]);
        while let Some((code, before)) = look.pop_front() {
            let near = code.start..code.end.min(code.start + FAR);
            for set in names(tokens, &near).filter_map(|(name, _)| self.last(&name, &code, before)) {
                if found.len() < MOST && !found.contains(&set.statement) {
                    found.push(set.statement.clone());
                    look.push_back((set.at + 1..set.statement.end, set.statement.start));
                }
            }
        }
        found
    }

    /// The statement that last set a name before some code, in sight of it.
    fn last(&self, name: &str, code: &Range<usize>, before: usize) -> Option<&Setting> {
        let settings = self.0.get(name)?;
        let earlier = &settings[..settings.partition_point(|s| s.statement.end <= before)];
        let in_sight = |s: &&Setting| s.seen.start <= code.start && code.end <= s.seen.end;
        earlier.iter().rev().take(FAR).find(in_sight)
    }
}

/// What the assignment at token `at` sets: the names before it in its statement, `x` and `y` of
/// `const [x, y] = z`, or in brackets of its own, the one before a `:=`; a default or a named
/// argument there sets nothing.
fn targets(tokens: &[Token], statement: &Range<usize>, at: usize) -> Vec<String> {
    let before = statement.start.min(at)..at;
    if before.len() > FAR {
        return Vec::new();
    }
    let depth = tokens[before.clone()].iter().fold(0, |depth, t| match t.text.as_str() {
        "(" | "[" | "{" => depth + 1,
        ")" | "]" | "}" => depth - 1,
        _ => depth,
    });
    let before = match (depth, tokens[at].text.as_str()) {
        (0, _) => before,
        (_, ":=") => at.saturating_sub(1)..at,
        _ => return Vec::new(),
    };
    let whole = names(tokens, &before).filter(|(_, whole)| *whole);
    whole.map(|(name, _)| name).collect()
}

/// The names in some code, `a` and `a.b` of `a.b`, and whether each is all of its own.
fn names<'t>(tokens: &'t [Token], code: &'t Range<usize>) -> impl Iterator<Item = (String, bool)> + 't {
    let dot = |i: usize| tokens.get(i).is_some_and(|t| t.text == ".");
    let word = |i: usize| {
        let first = tokens.get(i).and_then(|t| t.text.chars().next());
        first.is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
    };
    let starts = code.clone().filter(move |&i| word(i) && !(i > 0 && dot(i - 1)));
    starts.flat_map(move |start| {
        let mut name = tokens[start].text.clone();
        let mut all = Vec::new();
        let mut at = start;
        while dot(at + 1) && word(at + 2) && at + 2 < code.end && all.len() < MOST {
            all.push((name.clone(), false));
            name = format!("{name}.{}", tokens[at + 2].text);
            at += 2;
        }
        all.push((name, true));
        all
    })
}
