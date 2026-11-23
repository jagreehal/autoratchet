//! Functions that repeat a name get `#2`, `#3`, … in order, so a change that adds, removes or
//! moves one renames the others. Which of base's each of head's is, by its code.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hash;
use std::path::Path;

use crate::align::{Alignment, Function};
use crate::gate::FunctionSnapshot;
use crate::snapshots::Snapshots;

/// A file's functions of one name in the source, in one function, in base and in head.
type Family<'s> = [Vec<Named<'s>>; 2];

#[derive(Clone, Copy)]
struct Named<'s> {
    key: &'s str,
    snapshot: &'s FunctionSnapshot,
}

impl<'s> Named<'s> {
    fn source_name(&self) -> &'s str {
        let origin = self.snapshot.origin.as_ref();
        origin.map_or(&self.snapshot.function_name, |origin| &origin.source_name)
    }
}

/// One side's functions, by file and name.
struct Side<'s>(HashMap<(&'s str, &'s str), Named<'s>>);

impl<'s> Side<'s> {
    fn of(snapshots: &'s Snapshots) -> Self {
        let named = snapshots.iter().map(|(key, snapshot)| Named { key, snapshot });
        let by_name = |n: Named<'s>| ((n.snapshot.file_path.as_str(), n.snapshot.function_name.as_str()), n);
        Side(named.map(by_name).collect())
    }

    /// The function another is in.
    fn outer(&self, named: &Named<'s>) -> Option<&Named<'s>> {
        let outer = named.snapshot.origin.as_ref()?.outer.as_ref()?;
        self.0.get(&(named.snapshot.file_path.as_str(), outer.as_str()))
    }

    /// Those in `outer`, and for none those in no function.
    fn inside(&self) -> HashMap<Option<&'s str>, Vec<Named<'s>>> {
        let mut found: HashMap<_, Vec<Named>> = HashMap::new();
        for named in self.0.values() {
            let outer = self.outer(named).map(|outer| outer.key);
            found.entry(outer).or_default().push(*named);
        }
        // As the source has them.
        let start = |n: &Named| n.snapshot.span.as_ref().map(|span| span.start);
        let all = found.values_mut();
        all.for_each(|all| all.sort_by(|a, b| start(a).cmp(&start(b)).then(a.key.cmp(b.key))));
        found
    }
}

/// Put base's functions under the keys of head's that they are.
pub fn reconcile(
    previous: &mut Snapshots,
    current: &Snapshots,
    roots: (&Path, &Path),
    alignments: &mut HashMap<String, Alignment>,
) {
    // Every one of base's not paired below is gone.
    let mut keys: HashMap<String, Option<String>> = previous.iter().map(|(key, _)| (key.to_string(), None)).collect();
    let mut files = Files {
        roots,
        alignments,
        differ: HashMap::new(),
    };
    let sides = [Side::of(previous), Side::of(current)];
    let mut inside = sides.each_ref().map(Side::inside);
    // Functions of each pair pair with each other; those in no function, by file.
    let mut pairs: Vec<(Option<&str>, Option<&str>)> = vec![(None, None)];
    while let Some((old, new)) = pairs.pop() {
        let mut families: BTreeMap<(&str, &str), Family> = BTreeMap::new();
        for (side, outer) in [old, new].into_iter().enumerate() {
            for named in inside[side].remove(&outer).into_iter().flatten() {
                let family = (named.snapshot.file_path.as_str(), named.source_name());
                families.entry(family).or_default()[side].push(named);
            }
        }
        for ((file, _), [olds, news]) in families {
            for (old, new) in files.pairs(file, &olds, &news) {
                let (old, new) = (olds[old].key, news[new].key);
                keys.insert(old.to_string(), Some(new.to_string()));
                pairs.push((Some(old), Some(new)));
            }
        }
    }
    previous.rekey(&keys);
}

/// The files compared, each read as little as it takes.
struct Files<'a> {
    roots: (&'a Path, &'a Path),
    alignments: &'a mut HashMap<String, Alignment>,
    differ: HashMap<String, bool>,
}

impl Files<'_> {
    fn differs(&mut self, file: &str) -> bool {
        let read = |root: &Path| std::fs::read(root.join(file)).ok();
        let (base, head) = self.roots;
        *self
            .differ
            .entry(file.to_string())
            .or_insert_with(|| read(base) != read(head))
    }

    /// Which of base's functions of a family are which of head's.
    fn pairs(&mut self, file: &str, olds: &[Named], news: &[Named]) -> Vec<(usize, usize)> {
        if olds.is_empty() || news.is_empty() {
            return Vec::new();
        }
        // The same file has the same functions, in order.
        if olds.len() + news.len() == 2 || !self.differs(file) {
            return (0..olds.len()).zip(0..news.len()).collect();
        }
        let (base, head) = self.roots;
        let alignment = self
            .alignments
            .entry(file.to_string())
            .or_insert_with(|| Alignment::new(&base.join(file), &head.join(file)));
        let span = |named: &Named| named.snapshot.span.clone().unwrap_or_default();
        let old = olds.iter().map(|old| alignment.base_function(&span(old))).collect();
        let new = news.iter().map(|new| alignment.head_function(&span(new))).collect();
        Matching::of(alignment, old, new).pairs
    }
}

/// Which of one family's functions are which, so far.
struct Matching<'a> {
    alignment: &'a Alignment,
    old: Vec<Function>,
    new: Vec<Function>,
    /// Those not matched yet.
    old_left: Vec<usize>,
    new_left: Vec<usize>,
    pairs: Vec<(usize, usize)>,
}

/// The most tokens compared to tell which of the functions left in one place are alike.
// Beyond this they pair in order.
const MOST: usize = 20_000_000;

/// How much of two functions is the same, in thousandths, for them to be alike.
const ALIKE: usize = 500;

impl<'a> Matching<'a> {
    /// Pair functions, the surest way first: in the same place, with the most code around them
    /// whole, or else alike; then what was moved whole; then whatever is most alike in as much
    /// the same place as any.
    fn of(alignment: &'a Alignment, old: Vec<Function>, new: Vec<Function>) -> Self {
        let mut matching = Matching {
            old_left: (0..old.len()).collect(),
            new_left: (0..new.len()).collect(),
            alignment,
            old,
            new,
            pairs: Vec::new(),
        };
        let deepest = matching.old.iter().map(|f| f.within.len()).max().unwrap_or(0);
        for depth in (0..deepest).rev() {
            matching.alike(|f| f.within.get(depth).map(|within| within.in_place));
        }
        matching.alike(|f| Some((f.around[0], f.code)));
        matching.likest(0, ALIKE);
        for depth in (0..deepest).rev() {
            matching.alike(|f| f.within.get(depth).map(|within| within.alone));
        }
        for level in 0..Function::AROUND {
            matching.alike(|f| Some((f.around[level], f.code)));
            matching.likest(level, 0);
        }
        matching
    }

    /// Pair those of the same key: first those that kept their place.
    fn alike<K: Eq + Hash>(&mut self, key: impl Fn(&Function) -> Option<K>) {
        self.same(|f| Some((f.place?, key(f)?)));
        self.same(key);
    }

    fn same<K: Eq + Hash>(&mut self, key: impl Fn(&Function) -> Option<K>) {
        let mut free: HashMap<K, Vec<usize>> = HashMap::new();
        for &old in self.old_left.iter().rev() {
            if let Some(key) = key(&self.old[old]) {
                free.entry(key).or_default().push(old);
            }
        }
        let new = |&new: &usize| Some((key(&self.new[new]).and_then(|key| free.get_mut(&key)?.pop())?, new));
        let pairs: Vec<_> = self.new_left.iter().filter_map(new).collect();
        self.take(pairs);
    }

    /// Pair those in the same place as told at `level`, the most alike first; of those as alike,
    /// the ones that kept the most in place, then in order.
    fn likest(&mut self, level: usize, least: usize) {
        let mut groups: BTreeMap<u64, [Vec<usize>; 2]> = BTreeMap::new();
        for &old in &self.old_left {
            groups.entry(self.old[old].around[level]).or_default()[0].push(old);
        }
        for &new in &self.new_left {
            groups.entry(self.new[new].around[level]).or_default()[1].push(new);
        }
        let groups = groups.into_values();
        let pairs: Vec<_> = groups
            .flat_map(|[old, new]| self.by_likeness(&old, &new, least))
            .collect();
        self.take(pairs);
    }

    fn by_likeness(&self, old: &[usize], new: &[usize], least: usize) -> Vec<(usize, usize)> {
        let size = |of: &[Function], which: &[usize]| which.iter().map(|&i| of[i].size()).sum::<usize>();
        let work = size(&self.old, old) * new.len() + size(&self.new, new) * old.len();
        if work > MOST {
            if least > 0 {
                return Vec::new();
            }
            return old.iter().copied().zip(new.iter().copied()).collect();
        }
        let both = old.iter().flat_map(|&o| new.iter().map(move |&n| (o, n)));
        let mut both: Vec<_> = both
            .map(|(o, n)| (Reverse(self.likeness(o, n)), o, n))
            .filter(|(Reverse((alike, _)), ..)| *alike >= least)
            .collect();
        both.sort_unstable();
        let (mut olds, mut news) = (HashSet::new(), HashSet::new());
        let free = both.into_iter().map(|(_, o, n)| (o, n));
        free.filter(|(o, n)| !olds.contains(o) && !news.contains(n) && olds.insert(*o) && news.insert(*n))
            .collect()
    }

    /// In thousandths, and tokens.
    fn likeness(&self, old: usize, new: usize) -> (usize, usize) {
        let (old, new) = (&self.old[old], &self.new[new]);
        let both = (old.size() + new.size()).max(1);
        (
            2000 * self.alignment.shared(old, new) / both,
            self.alignment.kept(old, new),
        )
    }

    fn take(&mut self, pairs: Vec<(usize, usize)>) {
        let old: HashSet<usize> = pairs.iter().map(|(old, _)| *old).collect();
        let new: HashSet<usize> = pairs.iter().map(|(_, new)| *new).collect();
        self.old_left.retain(|i| !old.contains(i));
        self.new_left.retain(|i| !new.contains(i));
        self.pairs.extend(pairs);
    }
}
