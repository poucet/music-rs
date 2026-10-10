//! OSC address pattern matching, and the address space it matches into.
//!
//! A received message's address may be a *pattern*: `?` matches any single
//! character, `*` any sequence of zero or more characters, `[abc]` a
//! character in the set (`[a-z]` a range, `[!...]` the complement), and
//! `{foo,bar}` either literal string. None of them ever match `/` — a
//! pattern part matches exactly one address part, by spec. These are OSC
//! 1.0's rules (spec-1_0 "OSC Message Dispatching and Pattern Matching");
//! the 1.1 `//` any-depth operator is deliberately not implemented, and a
//! `//` in a pattern simply never matches (an empty part cannot match a
//! named one).
//!
//! The spec's sharp corners, implemented as written:
//! * a `-` at the start or end of a `[]` set is itself, not a range;
//! * a `!` anywhere but first in a `[]` set is itself, not negation;
//! * a `]` first in a set would end it immediately — OSC defines no
//!   escape, so `[]` is an empty set and matches nothing;
//! * an unterminated `[` or `{` is a malformed pattern and matches
//!   nothing (refusing beats guessing on untrusted input);
//! * a reversed range (`[z-a]`) contains nothing.
//!
//! [`AddressSpace`] is the dispatch half: registered addresses are plain
//! (no pattern characters — the spec reserves them in method names), and
//! [`AddressSpace::matching`] walks them against a received pattern. This
//! is the piece a binding layer leans on.

/// Does `pattern` match `address`, under OSC 1.0 pattern matching?
///
/// `address` is a plain address (as registered); `pattern` is what a
/// message carries. A plain pattern matches only itself.
pub fn matches(pattern: &str, address: &str) -> bool {
    let mut failed = std::collections::HashSet::new();
    match_at(pattern.as_bytes(), address.as_bytes(), 0, 0, &mut failed)
}

/// Backtracking matcher over positions `(pi, ai)` into pattern and
/// address. Patterns come off the network, so the worst case matters: a
/// pattern of repeated `*`s against a repetitive address would backtrack
/// exponentially, which from UDP is a denial of service. `failed`
/// memoizes every position pair that has already failed, bounding the
/// work to O(pattern × address) states. Malformed syntax returns false.
fn match_at(
    p: &[u8],
    a: &[u8],
    pi: usize,
    ai: usize,
    failed: &mut std::collections::HashSet<(usize, usize)>,
) -> bool {
    if failed.contains(&(pi, ai)) {
        return false;
    }
    let ok = match_at_uncached(p, a, pi, ai, failed);
    if !ok {
        failed.insert((pi, ai));
    }
    ok
}

fn match_at_uncached(
    p: &[u8],
    a: &[u8],
    pi: usize,
    ai: usize,
    failed: &mut std::collections::HashSet<(usize, usize)>,
) -> bool {
    match p.get(pi) {
        None => ai == a.len(),
        Some(b'?') => match a.get(ai) {
            Some(&c) if c != b'/' => match_at(p, a, pi + 1, ai + 1, failed),
            _ => false,
        },
        Some(b'*') => {
            // Zero or more non-'/' characters: try the shortest first.
            if match_at(p, a, pi + 1, ai, failed) {
                return true;
            }
            for i in ai..a.len() {
                if a[i] == b'/' {
                    return false;
                }
                if match_at(p, a, pi + 1, i + 1, failed) {
                    return true;
                }
            }
            false
        }
        Some(b'[') => {
            let Some(end) = p[pi + 1..].iter().position(|&c| c == b']') else {
                return false; // unterminated set: malformed, matches nothing
            };
            let set = &p[pi + 1..pi + 1 + end];
            match a.get(ai) {
                Some(&c) if c != b'/' && in_set(set, c) => {
                    match_at(p, a, pi + end + 2, ai + 1, failed)
                }
                _ => false,
            }
        }
        Some(b'{') => {
            let Some(end) = p[pi + 1..].iter().position(|&c| c == b'}') else {
                return false; // unterminated alternation: matches nothing
            };
            let alts = &p[pi + 1..pi + 1 + end];
            alts.split(|&c| c == b',').any(|alt| {
                a.len() - ai >= alt.len()
                    && &a[ai..ai + alt.len()] == alt
                    && match_at(p, a, pi + end + 2, ai + alt.len(), failed)
            })
        }
        Some(&c) => match a.get(ai) {
            Some(&d) if d == c => match_at(p, a, pi + 1, ai + 1, failed),
            _ => false,
        },
    }
}

/// Is `c` in the `[]` set `set` (the bytes between the brackets)?
fn in_set(set: &[u8], c: u8) -> bool {
    let (negated, set) = match set.split_first() {
        Some((b'!', rest)) => (true, rest),
        _ => (false, set),
    };
    let mut found = false;
    let mut i = 0;
    while i < set.len() {
        // A range needs a character on both sides: `a-z` with the `-`
        // neither first nor last.
        if i + 2 < set.len() && set[i + 1] == b'-' {
            if set[i] <= c && c <= set[i + 2] {
                found = true;
            }
            i += 3;
        } else {
            if set[i] == c {
                found = true;
            }
            i += 1;
        }
    }
    found != negated
}

/// Why an address cannot be registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressError {
    /// An address begins with `/`.
    NoLeadingSlash,
    /// The character is reserved for patterns (` # * , ? [ ] { }`) or is
    /// a control character, and cannot appear in a registered address.
    ReservedCharacter(char),
}

impl std::fmt::Display for AddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressError::NoLeadingSlash => write!(f, "an address begins with '/'"),
            AddressError::ReservedCharacter(c) => {
                write!(f, "{c:?} is reserved and cannot appear in an address")
            }
        }
    }
}

impl std::error::Error for AddressError {}

/// Validate a plain (pattern-free) address: leading `/`, none of the
/// characters the spec reserves for patterns, no control characters.
pub fn validate_address(address: &str) -> Result<(), AddressError> {
    if !address.starts_with('/') {
        return Err(AddressError::NoLeadingSlash);
    }
    for c in address.chars() {
        if matches!(c, ' ' | '#' | '*' | ',' | '?' | '[' | ']' | '{' | '}') || c.is_control() {
            return Err(AddressError::ReservedCharacter(c));
        }
    }
    Ok(())
}

/// A registered address space: plain addresses, each carrying a value —
/// a binding, a handler, an id — matched against received patterns.
///
/// Linear scan on match: an address space here is tens of knobs, not a
/// routing table, and a scan over a `Vec` beats a trie until it is not.
#[derive(Clone, Debug, Default)]
pub struct AddressSpace<T> {
    entries: Vec<(String, T)>,
}

impl<T> AddressSpace<T> {
    pub fn new() -> Self {
        AddressSpace { entries: Vec::new() }
    }

    /// Register `address` with `value`. The address must be plain — see
    /// [`validate_address`]. Re-registering an address replaces its value.
    pub fn register(&mut self, address: &str, value: T) -> Result<(), AddressError> {
        validate_address(address)?;
        match self.entries.iter_mut().find(|(a, _)| a == address) {
            Some((_, v)) => *v = value,
            None => self.entries.push((address.to_owned(), value)),
        }
        Ok(())
    }

    /// Remove an address; the value it carried, if it was registered.
    pub fn unregister(&mut self, address: &str) -> Option<T> {
        let i = self.entries.iter().position(|(a, _)| a == address)?;
        Some(self.entries.remove(i).1)
    }

    /// Exact lookup, no pattern semantics.
    pub fn get(&self, address: &str) -> Option<&T> {
        self.entries.iter().find(|(a, _)| a == address).map(|(_, v)| v)
    }

    /// Every registered address the received `pattern` matches, in
    /// registration order.
    pub fn matching<'a>(&'a self, pattern: &'a str) -> impl Iterator<Item = (&'a str, &'a T)> {
        self.entries.iter().filter(move |(a, _)| matches(pattern, a)).map(|(a, v)| (a.as_str(), v))
    }

    /// How many addresses are registered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_pattern_matches_only_itself() {
        assert!(matches("/a/b", "/a/b"));
        assert!(!matches("/a/b", "/a/c"));
        assert!(!matches("/a/b", "/a/b/c"));
        assert!(!matches("/a/b/c", "/a/b"));
        assert!(!matches("/a", ""));
        assert!(matches("", ""));
    }

    #[test]
    fn question_mark_matches_any_single_character_except_slash() {
        assert!(matches("/a?c", "/abc"));
        assert!(matches("/a?c", "/a?c"));
        assert!(!matches("/a?c", "/ac")); // not zero characters
        assert!(!matches("/a?c", "/abbc")); // not two
        assert!(!matches("/a?c", "/a/c")); // never '/'
    }

    #[test]
    fn star_matches_any_run_except_slash() {
        assert!(matches("/*", "/anything"));
        assert!(matches("/a*", "/a")); // zero characters
        assert!(matches("/*/gain", "/osc/gain"));
        assert!(!matches("/*", "/a/b")); // never crosses '/'
        assert!(matches("/a*c*e", "/abcde"));
        assert!(matches("/a*c*e", "/ace"));
        assert!(!matches("/a*c*e", "/abde"));
    }

    #[test]
    fn brackets_match_a_set_a_range_and_their_complement() {
        assert!(matches("/[abc]", "/b"));
        assert!(!matches("/[abc]", "/d"));
        assert!(matches("/[a-c]", "/b"));
        assert!(!matches("/[a-c]", "/d"));
        assert!(matches("/fader[1-8]", "/fader3"));
        assert!(!matches("/fader[1-8]", "/fader9"));
        assert!(matches("/[!abc]", "/d"));
        assert!(!matches("/[!abc]", "/a"));
        assert!(!matches("/[abc]", "/ab")); // one character exactly
    }

    #[test]
    fn bracket_corners_from_the_spec_text() {
        // '-' at the end is itself, not a range.
        assert!(matches("/[a-]", "/-"));
        assert!(matches("/[a-]", "/a"));
        assert!(!matches("/[a-]", "/b"));
        // '-' at the start is itself.
        assert!(matches("/[-a]", "/-"));
        // '!' anywhere but first is itself.
        assert!(matches("/[a!]", "/!"));
        assert!(matches("/[a!]", "/a"));
        // A negated set still never matches '/'.
        assert!(!matches("/a/[!x]", "/a//"));
        // A reversed range contains nothing.
        assert!(!matches("/[z-a]", "/m"));
        // An empty set matches nothing.
        assert!(!matches("/[]", "/a"));
        // Unterminated syntax matches nothing rather than guessing.
        assert!(!matches("/[abc", "/a"));
        assert!(!matches("/{ab", "/ab"));
    }

    #[test]
    fn braces_match_either_literal() {
        assert!(matches("/{foo,bar}/x", "/foo/x"));
        assert!(matches("/{foo,bar}/x", "/bar/x"));
        assert!(!matches("/{foo,bar}/x", "/baz/x"));
        // Zero-length alternative is legal: {,abc} matches the empty string too.
        assert!(matches("/a{,bc}", "/a"));
        assert!(matches("/a{,bc}", "/abc"));
        // Alternatives are literals; a ',' splits even a wished-for nest.
        assert!(matches("/{a,b}{1,2}", "/b2"));
    }

    #[test]
    fn the_one_slash_rule_binds_every_operator_to_one_part() {
        // The spec's own framing: a pattern part matches one address part.
        assert!(matches("/voice/?/freq", "/voice/1/freq"));
        assert!(!matches("/voice/*", "/voice/1/freq"));
        assert!(matches("/voice/*/freq", "/voice/anything/freq"));
    }

    #[test]
    fn double_slash_is_1_1_and_never_matches_here() {
        assert!(!matches("//freq", "/voice/freq"));
        assert!(!matches("/a//b", "/a/x/b"));
        // ...but a literal double slash in both still compares equal,
        // because an empty part matches an empty part.
        assert!(matches("/a//b", "/a//b"));
    }

    #[test]
    fn a_hostile_star_pattern_is_not_exponential() {
        // Twenty alternating `*a` units against forty `a`s: the naive
        // backtracker is 2^40 here; the memoized one is linear-ish.
        let pattern = format!("/{}b", "*a".repeat(20));
        let address = format!("/{}", "a".repeat(40));
        let t = std::time::Instant::now();
        assert!(!matches(&pattern, &address));
        assert!(t.elapsed() < std::time::Duration::from_secs(1), "took {:?}", t.elapsed());
    }

    #[test]
    fn address_space_registers_looks_up_and_dispatches() {
        let mut space = AddressSpace::new();
        space.register("/synth/freq", 1).unwrap();
        space.register("/synth/amp", 2).unwrap();
        space.register("/drum/amp", 3).unwrap();
        assert_eq!(space.get("/synth/amp"), Some(&2));
        assert_eq!(space.get("/synth/nope"), None);
        let hits: Vec<_> = space.matching("/synth/*").collect();
        assert_eq!(hits, vec![("/synth/freq", &1), ("/synth/amp", &2)]);
        let hits: Vec<_> = space.matching("/*/amp").collect();
        assert_eq!(hits, vec![("/synth/amp", &2), ("/drum/amp", &3)]);
        assert_eq!(space.unregister("/synth/amp"), Some(2));
        assert_eq!(space.matching("/synth/*").count(), 1);
    }

    #[test]
    fn registration_replaces_and_rejects_patterns() {
        let mut space = AddressSpace::new();
        space.register("/a", 1).unwrap();
        space.register("/a", 2).unwrap();
        assert_eq!(space.len(), 1);
        assert_eq!(space.get("/a"), Some(&2));
        assert_eq!(space.register("a", 0), Err(AddressError::NoLeadingSlash));
        assert_eq!(space.register("/a*", 0), Err(AddressError::ReservedCharacter('*')));
        assert_eq!(space.register("/a b", 0), Err(AddressError::ReservedCharacter(' ')));
        assert_eq!(space.register("/a\0", 0), Err(AddressError::ReservedCharacter('\0')));
    }
}
