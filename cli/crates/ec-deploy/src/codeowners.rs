//! CODEOWNERS matching (DESIGN-cli §8.4; deck ch. 13 slice 5 — access control; REVIEW #10).
//!
//! Access control in the Studio is a **rendering of Git-host review state**, never a parallel
//! approval system: who must review a change to a layer is exactly who a `CODEOWNERS` rule assigns
//! to that file, plus whatever branch protection the host enforces. This module is the pure half —
//! parse the file's text and answer "who owns this path" — with no I/O; the adapter finds and reads
//! the file, and the Studio pairs the answer with the definition's scopes.
//!
//! The matcher implements the CODEOWNERS pattern subset GitHub and GitLab share: `#` comments,
//! last-match-wins, gitignore-style globs (`*` within a segment, `**` across segments, a trailing
//! `/` for a directory, a leading `/` or an internal slash to anchor at the repo root, and a
//! slashless pattern that matches at any depth). It does not model owner *validity* (whether a
//! handle exists) — that is the host's job, and the Studio never invents it.

/// A parsed `CODEOWNERS` file: its rules in file order. Matching is last-match-wins, so the rules
/// are consulted back to front.
#[derive(Debug, Clone, Default)]
pub struct CodeOwners {
    rules: Vec<Rule>,
}

#[derive(Debug, Clone)]
struct Rule {
    /// The pattern as authored, kept verbatim for display ("which line owns this file").
    pattern: String,
    owners: Vec<String>,
    regex: Regex,
}

impl CodeOwners {
    /// Parse `CODEOWNERS` text. Malformed lines (a pattern that cannot compile) are skipped rather
    /// than failing the whole file — a hostile line must not blind the Studio to every other rule.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut rules = Vec::new();
        for raw in text.lines() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(pattern) = parts.next() else {
                continue;
            };
            let owners: Vec<String> = parts.map(str::to_string).collect();
            if let Some(regex) = Regex::compile(pattern) {
                rules.push(Rule {
                    pattern: pattern.to_string(),
                    owners,
                    regex,
                });
            }
        }
        Self { rules }
    }

    /// The last rule matching `path` (a repo-relative, forward-slashed path, no leading slash), or
    /// `None` when no rule matches — which the caller renders as "falls to default branch
    /// protection", never as "unrestricted".
    #[must_use]
    pub fn owner_of(&self, path: &str) -> Option<Match<'_>> {
        let path = path.trim_start_matches('/');
        self.rules
            .iter()
            .rev()
            .find(|r| r.regex.is_match(path))
            .map(|r| Match {
                pattern: &r.pattern,
                owners: &r.owners,
            })
    }

    /// Whether the file carried any rule at all — an empty (or comment-only) `CODEOWNERS` is
    /// reported honestly rather than as "everything unowned".
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// A resolved ownership answer: the rule that matched and who it assigns.
#[derive(Debug, Clone, Copy)]
pub struct Match<'a> {
    pub pattern: &'a str,
    pub owners: &'a [String],
}

/// Drop a trailing `#` comment. A `#` is only a comment at the start of a token (after
/// whitespace); CODEOWNERS never uses `#` inside a pattern, so a plain find is faithful.
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

/// A minimal glob→matcher for CODEOWNERS patterns. Anchored patterns match from the repo root;
/// slashless patterns match a path component at any depth. Compiled once per rule.
#[derive(Debug, Clone)]
struct Regex {
    /// Segments of the compiled matcher, alternating literal and wildcard, evaluated against the
    /// candidate as a single left-anchored, right-anchored walk. We avoid a regex dependency and
    /// implement the tiny subset directly, which keeps the semantics auditable.
    matcher: Matcher,
}

#[derive(Debug, Clone)]
enum Matcher {
    /// A compiled sequence of tokens matched against the whole path.
    Tokens { anchored: bool, tokens: Vec<Tok> },
}

/// One token in a compiled pattern.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// A literal run containing no wildcard and no slash (one path-segment fragment).
    Lit(String),
    /// `/` — a path separator.
    Slash,
    /// `*` — any run of non-slash characters (possibly empty).
    Star,
    /// `**` — any run of characters including slashes (possibly empty).
    DoubleStar,
    /// `**/` — zero or more **whole** directory segments (each ending in `/`), so `a/**/b` matches
    /// `a/b` with no intermediate directory, the gitignore semantics Git hosts use.
    DoubleStarDir,
}

impl Regex {
    /// Compile a CODEOWNERS pattern, or `None` if it is empty after normalization.
    fn compile(pattern: &str) -> Option<Self> {
        let trimmed = pattern.trim();
        if trimmed.is_empty() {
            return None;
        }
        // A trailing slash means "this directory and everything under it".
        let dir = trimmed.ends_with('/');
        let core = trimmed.trim_end_matches('/');
        if core.is_empty() {
            return None;
        }
        // Anchored when it starts at root or carries an internal slash; else it matches at any depth.
        let anchored = core.starts_with('/') || core.contains('/');
        let core = core.trim_start_matches('/');

        let mut tokens = Vec::new();
        let mut chars = core.chars().peekable();
        let mut lit = String::new();
        let flush = |lit: &mut String, tokens: &mut Vec<Tok>| {
            if !lit.is_empty() {
                tokens.push(Tok::Lit(std::mem::take(lit)));
            }
        };
        while let Some(c) = chars.next() {
            match c {
                '/' => {
                    flush(&mut lit, &mut tokens);
                    tokens.push(Tok::Slash);
                }
                '*' => {
                    flush(&mut lit, &mut tokens);
                    if chars.peek() == Some(&'*') {
                        chars.next();
                        // `**/` is "zero or more directory segments"; a bare `**` is "any run".
                        if chars.peek() == Some(&'/') {
                            chars.next();
                            tokens.push(Tok::DoubleStarDir);
                        } else {
                            tokens.push(Tok::DoubleStar);
                        }
                    } else {
                        tokens.push(Tok::Star);
                    }
                }
                other => lit.push(other),
            }
        }
        flush(&mut lit, &mut tokens);
        // A directory pattern matches everything beneath it: append `/**`.
        if dir {
            tokens.push(Tok::Slash);
            tokens.push(Tok::DoubleStar);
        }
        Some(Self {
            matcher: Matcher::Tokens { anchored, tokens },
        })
    }

    fn is_match(&self, path: &str) -> bool {
        let Matcher::Tokens { anchored, tokens } = &self.matcher;
        if *anchored {
            match_tokens(tokens, path)
        } else {
            // Unanchored: match at the root or after any `/` boundary.
            if match_tokens(tokens, path) {
                return true;
            }
            let bytes: Vec<usize> = path
                .char_indices()
                .filter(|(_, c)| *c == '/')
                .map(|(i, _)| i + 1)
                .collect();
            bytes
                .into_iter()
                .any(|start| match_tokens(tokens, &path[start..]))
        }
    }
}

/// Match a token sequence against a path prefix-anchored (the whole path must be consumed).
/// Backtracking is bounded by the tiny token count per rule.
fn match_tokens(tokens: &[Tok], path: &str) -> bool {
    match tokens.split_first() {
        None => path.is_empty(),
        Some((Tok::Lit(s), rest)) => path
            .strip_prefix(s.as_str())
            .is_some_and(|r| match_tokens(rest, r)),
        Some((Tok::Slash, rest)) => path
            .strip_prefix('/')
            .is_some_and(|r| match_tokens(rest, r)),
        Some((Tok::Star, rest)) => {
            // Any run of non-slash characters, shortest first.
            let limit = path.find('/').unwrap_or(path.len());
            (0..=limit)
                .filter(|i| path.is_char_boundary(*i))
                .any(|i| match_tokens(rest, &path[i..]))
        }
        Some((Tok::DoubleStar, rest)) => (0..=path.len())
            .filter(|i| path.is_char_boundary(*i))
            .any(|i| match_tokens(rest, &path[i..])),
        Some((Tok::DoubleStarDir, rest)) => {
            // Consume zero directories (rest at the current position) or any whole number of
            // leading `segment/` runs (rest at each position just past a `/`).
            if match_tokens(rest, path) {
                return true;
            }
            path.char_indices()
                .filter(|(_, c)| *c == '/')
                .any(|(i, _)| match_tokens(rest, &path[i + 1..]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owners<'a>(co: &'a CodeOwners, path: &str) -> Vec<&'a str> {
        co.owner_of(path)
            .map(|m| m.owners.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    #[test]
    fn last_match_wins() {
        let co = CodeOwners::parse(
            "* @plant-eng\nlayers/ @config-team\nlayers/components/telemetry.json @telemetry-owner\n",
        );
        assert_eq!(owners(&co, "README.md"), vec!["@plant-eng"]);
        assert_eq!(owners(&co, "layers/scopes/site.json"), vec!["@config-team"]);
        assert_eq!(
            owners(&co, "layers/components/telemetry.json"),
            vec!["@telemetry-owner"]
        );
    }

    #[test]
    fn directory_pattern_covers_everything_under_it() {
        let co = CodeOwners::parse("sites/dallas-site/ @dallas-leads\n");
        assert_eq!(
            owners(&co, "sites/dallas-site/definition.yaml"),
            vec!["@dallas-leads"]
        );
        assert_eq!(
            owners(&co, "sites/dallas-site/layers/components/x.json"),
            vec!["@dallas-leads"]
        );
        assert!(owners(&co, "sites/other/definition.yaml").is_empty());
    }

    #[test]
    fn slashless_pattern_matches_at_any_depth() {
        let co = CodeOwners::parse("*.json @json-owners\n");
        assert_eq!(owners(&co, "a/b/c.json"), vec!["@json-owners"]);
        assert_eq!(owners(&co, "top.json"), vec!["@json-owners"]);
        assert!(owners(&co, "a/b/c.yaml").is_empty());
    }

    #[test]
    fn double_star_crosses_segments_but_star_does_not() {
        let co = CodeOwners::parse("/layers/**/telemetry.json @deep\n/one/*/only.json @shallow\n");
        assert_eq!(owners(&co, "layers/a/b/telemetry.json"), vec!["@deep"]);
        assert_eq!(owners(&co, "layers/telemetry.json"), vec!["@deep"]);
        assert_eq!(owners(&co, "one/x/only.json"), vec!["@shallow"]);
        assert!(owners(&co, "one/x/y/only.json").is_empty());
    }

    #[test]
    fn anchored_root_pattern_does_not_match_deeper() {
        let co = CodeOwners::parse("/config.json @root-only\n");
        assert_eq!(owners(&co, "config.json"), vec!["@root-only"]);
        assert!(owners(&co, "nested/config.json").is_empty());
    }

    #[test]
    fn multiple_owners_on_one_rule() {
        let co = CodeOwners::parse("* @a @b @team/c\n");
        assert_eq!(owners(&co, "x"), vec!["@a", "@b", "@team/c"]);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let co = CodeOwners::parse("# top comment\n\n* @all  # trailing\n");
        assert!(!co.is_empty());
        assert_eq!(owners(&co, "x"), vec!["@all"]);
    }

    #[test]
    fn an_empty_file_owns_nothing() {
        let co = CodeOwners::parse("# only a comment\n");
        assert!(co.is_empty());
        assert!(co.owner_of("anything").is_none());
    }

    #[test]
    fn the_matched_pattern_is_reported_for_display() {
        let co = CodeOwners::parse("* @all\nlayers/ @cfg\n");
        let m = co.owner_of("layers/x.json").unwrap();
        assert_eq!(m.pattern, "layers/");
    }
}
