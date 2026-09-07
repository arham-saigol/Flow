use std::collections::HashMap;
use unicode_categories::UnicodeCategories;
use unicode_normalization::UnicodeNormalization;

/// Normalizes matching keys with Unicode NFC, lowercase, and collapsed whitespace.
pub fn normalize_key(value: &str) -> String {
    let nfc: String = value.nfc().collect();
    let lower: String = nfc.to_lowercase();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Checks if a character continues a word.
/// Word continuation includes Unicode letters, numbers, combining marks, and connector punctuation (including underscore).
pub fn is_word_continuation(c: char) -> bool {
    c.is_letter() || c.is_number() || c.is_mark() || c.is_punctuation_connector()
}

/// Normalizes snippet triggers and utterances for snippet comparison.
/// Strips only sentence-final punctuation and outer quote/bracket wrappers,
/// preserving leading dots (.NET), trailing hashes (C#), plus signs (C++), etc.
/// Returns None if the string contains no letters or numbers (rejects punctuation-only triggers).
pub fn normalize_snippet_trigger(value: &str) -> Option<String> {
    let nfc: String = value.nfc().collect();
    let lower: String = nfc.to_lowercase();
    let collapsed = lower.split_whitespace().collect::<Vec<_>>().join(" ");

    // Outer quotes and brackets
    const OPEN_WRAPPERS: &[char] = &[
        '"', '\'', '(', '[', '{', '“', '‘', '¿', '¡', '«', '‹', '「', '『',
    ];
    const CLOSE_WRAPPERS: &[char] = &['"', '\'', ')', ']', '}', '”', '’', '»', '›', '」', '』'];
    const SENTENCE_FINAL: &[char] = &[
        '.', ',', '!', '?', ';', ':', '…', '。', '！', '？', '؟', '؛',
    ];

    let mut trimmed = collapsed.as_str();

    // Trim outer wrappers and sentence-final punctuation iteratively
    loop {
        let before_len = trimmed.len();
        trimmed =
            trimmed.trim_start_matches(|c: char| c.is_whitespace() || OPEN_WRAPPERS.contains(&c));

        // Strip leading sentence-final punctuation UNLESS it's a '.' followed immediately by an alphanumeric (e.g. .NET)
        let mut chars = trimmed.chars();
        if let Some(first) = chars.next() {
            if SENTENCE_FINAL.contains(&first) {
                let is_dot_identifier =
                    first == '.' && chars.next().map(|c| c.is_alphanumeric()).unwrap_or(false);
                if !is_dot_identifier {
                    trimmed = &trimmed[first.len_utf8()..];
                }
            }
        }

        trimmed = trimmed.trim_end_matches(|c: char| {
            c.is_whitespace() || CLOSE_WRAPPERS.contains(&c) || SENTENCE_FINAL.contains(&c)
        });
        if trimmed.len() == before_len {
            break;
        }
    }

    // Require at least one letter or number
    if !trimmed.chars().any(|c| c.is_letter() || c.is_number()) {
        return None;
    }

    Some(trimmed.to_string())
}

/// Normalizes correction sources for dictionary matching:
/// trims wrappers and punctuation from the ends, collapses whitespace, and converts to lowercase.
pub fn normalize_correction_source(value: &str) -> String {
    let nfc: String = value.nfc().collect();
    let lower: String = nfc.to_lowercase();
    lower
        .trim_start_matches(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '"' | '\'' | '(' | '[' | '{' | '“' | '‘' | '¿' | '¡'
                )
        })
        .trim_end_matches(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '.' | ','
                        | '!'
                        | '?'
                        | ';'
                        | ':'
                        | '"'
                        | '\''
                        | ')'
                        | ']'
                        | '}'
                        | '…'
                        | '”'
                        | '’'
                )
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone)]
pub struct ValidatedCorrection {
    pub normalized_source: String,
    pub replacement: String,
}

/// Takes a list of (source, replacement) pairs, filters out duplicates/conflicts,
/// and returns unambiguous mappings.
pub fn filter_conflicting_corrections(
    rules: &[(String, String)],
) -> (Vec<ValidatedCorrection>, Vec<String>) {
    // Map from normalized source -> (replacement, is_conflict)
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for (src, rep) in rules {
        let norm = normalize_key(src);
        if norm.is_empty() {
            continue;
        }
        groups.entry(norm).or_default().push(rep.clone());
    }

    let mut valid = Vec::new();
    let mut conflicts = Vec::new();

    for (norm, reps) in groups {
        if reps.len() > 1 {
            // Multiple rules with the same normalized source is a conflict
            conflicts.push(norm);
        } else if let Some(rep) = reps.into_iter().next() {
            valid.push(ValidatedCorrection {
                normalized_source: norm,
                replacement: rep,
            });
        }
    }

    // Sort by descending source length for longest-match-first priority
    valid.sort_by(|a, b| b.normalized_source.len().cmp(&a.normalized_source.len()));
    (valid, conflicts)
}

fn match_at(text: &str, start_byte: usize, rule_norm: &str) -> Option<usize> {
    let first_rule_char = rule_norm.chars().next()?;
    if is_word_continuation(first_rule_char) && start_byte > 0 {
        let prev_char = text[..start_byte].chars().next_back()?;
        if is_word_continuation(prev_char) {
            return None;
        }
    }

    let mut text_chars = text[start_byte..].char_indices().peekable();
    let mut rule_chars = rule_norm.chars().peekable();
    let mut last_matched_end = 0;

    while let Some(&rule_c) = rule_chars.peek() {
        if rule_c == ' ' {
            let mut saw_ws = false;
            while let Some(&(_, tc)) = text_chars.peek() {
                if tc.is_whitespace() {
                    saw_ws = true;
                    let (idx, c) = text_chars.next().unwrap();
                    last_matched_end = idx + c.len_utf8();
                } else {
                    break;
                }
            }
            if !saw_ws {
                return None;
            }
            rule_chars.next();
        } else {
            let &(t_idx, tc) = text_chars.peek()?;
            if tc.is_whitespace() {
                return None;
            }
            let tc_norm: String = tc.to_lowercase().collect();
            for norm_c in tc_norm.chars() {
                if rule_chars.peek() == Some(&norm_c) {
                    rule_chars.next();
                } else {
                    return None;
                }
            }
            text_chars.next();
            last_matched_end = t_idx + tc.len_utf8();
        }
    }

    let last_rule_char = rule_norm.chars().next_back()?;
    let matched_end_byte = start_byte + last_matched_end;
    if is_word_continuation(last_rule_char) && matched_end_byte < text.len() {
        let next_char = text[matched_end_byte..].chars().next()?;
        if is_word_continuation(next_char) {
            return None;
        }
    }

    Some(matched_end_byte)
}

/// Applies validated dictionary corrections to original text.
/// Uses normalized-to-original byte span matching, collapsing whitespace only,
/// preserving literal punctuation and formatting outside replacements.
pub fn apply_corrections(text: &str, rules: &[ValidatedCorrection]) -> String {
    if text.is_empty() || rules.is_empty() {
        return text.to_string();
    }

    #[derive(Debug)]
    struct Match {
        orig_start: usize,
        orig_end: usize,
        replacement: String,
        rule_len: usize,
    }

    let mut matches = Vec::new();

    for rule in rules {
        if rule.normalized_source.is_empty() {
            continue;
        }

        for (byte_idx, _) in text.char_indices() {
            if let Some(end_byte) = match_at(text, byte_idx, &rule.normalized_source) {
                matches.push(Match {
                    orig_start: byte_idx,
                    orig_end: end_byte,
                    replacement: rule.replacement.clone(),
                    rule_len: rule.normalized_source.len(),
                });
            }
        }
    }

    // Sort matches: left-to-right, then longest rule length first
    matches.sort_by(|a, b| {
        a.orig_start
            .cmp(&b.orig_start)
            .then_with(|| b.rule_len.cmp(&a.rule_len))
            .then_with(|| (b.orig_end - b.orig_start).cmp(&(a.orig_end - a.orig_start)))
    });

    let mut result = String::with_capacity(text.len());
    let mut last_idx = 0;

    for m in matches {
        if m.orig_start < last_idx {
            continue;
        }
        result.push_str(&text[last_idx..m.orig_start]);
        result.push_str(&m.replacement);
        last_idx = m.orig_end;
    }
    result.push_str(&text[last_idx..]);

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_key() {
        assert_eq!(normalize_key("  Hello   World  "), "hello world");
        assert_eq!(normalize_key("C#"), "c#");
        assert_eq!(normalize_key(".NET"), ".net");
    }

    #[test]
    fn test_word_continuation() {
        assert!(is_word_continuation('a'));
        assert!(is_word_continuation('9'));
        assert!(is_word_continuation('_'));
        assert!(!is_word_continuation(' '));
        assert!(!is_word_continuation('.'));
        assert!(!is_word_continuation('#'));
    }

    #[test]
    fn test_normalize_snippet_trigger() {
        assert_eq!(
            normalize_snippet_trigger("  My   EMAIL address... "),
            Some("my email address".into())
        );
        assert_eq!(normalize_snippet_trigger("。状态؟"), Some("状态".into()));
        assert_eq!(normalize_snippet_trigger("C#"), Some("c#".into()));
        assert_eq!(normalize_snippet_trigger(".NET"), Some(".net".into()));
        assert_eq!(normalize_snippet_trigger("C++"), Some("c++".into()));
        assert_eq!(normalize_snippet_trigger("..."), None);
        assert_eq!(normalize_snippet_trigger("!??"), None);
    }

    #[test]
    fn test_apply_corrections_boundary_and_preservation() {
        let rules = vec![
            ValidatedCorrection {
                normalized_source: "four word".into(),
                replacement: "Forward".into(),
            },
            ValidatedCorrection {
                normalized_source: "see sharp".into(),
                replacement: "C#".into(),
            },
            ValidatedCorrection {
                normalized_source: "c".into(),
                replacement: "see".into(),
            },
        ];

        // Preservation of surrounding punctuation and casing
        assert_eq!(
            apply_corrections("Please, four word now.", &rules),
            "Please, Forward now."
        );
        // Boundary check: "four words" should not match "four word"
        assert_eq!(apply_corrections("four words", &rules), "four words");
        // Underscore is a word continuation: "my_c_project" -> "c" inside identifier should not match
        assert_eq!(apply_corrections("my_c_project", &rules), "my_c_project");
        // Standalone "c" matches
        assert_eq!(apply_corrections("a c project", &rules), "a see project");
    }

    #[test]
    fn test_conflict_detection() {
        let rules = vec![
            ("btw".into(), "by the way".into()),
            ("BTW".into(), "back to work".into()), // conflict!
            ("c sharp".into(), "C#".into()),
        ];
        let (valid, conflicts) = filter_conflicting_corrections(&rules);
        assert_eq!(conflicts, vec!["btw".to_string()]);
        assert_eq!(valid.len(), 1);
        assert_eq!(valid[0].normalized_source, "c sharp");
    }

    #[test]
    fn test_probe_26_and_27_literal_corrections() {
        let rules = vec![
            ValidatedCorrection {
                normalized_source: normalize_key("C#"),
                replacement: "CSharp".into(),
            },
            ValidatedCorrection {
                normalized_source: normalize_key(".NET"),
                replacement: "DotNet".into(),
            },
            ValidatedCorrection {
                normalized_source: normalize_key("don't"),
                replacement: "do not".into(),
            },
            ValidatedCorrection {
                normalized_source: normalize_key("foo-bar"),
                replacement: "foobar".into(),
            },
            ValidatedCorrection {
                normalized_source: normalize_key("four word"),
                replacement: "forward".into(),
            },
        ];

        // Probe 26: literal sources C#, .NET, don't, foo-bar match identical input
        assert_eq!(
            apply_corrections("I write C# code.", &rules),
            "I write CSharp code."
        );
        assert_eq!(
            apply_corrections("Using .NET framework.", &rules),
            "Using DotNet framework."
        );
        assert_eq!(
            apply_corrections("Please don't do that.", &rules),
            "Please do not do that."
        );
        assert_eq!(
            apply_corrections("Testing foo-bar here.", &rules),
            "Testing foobar here."
        );

        // Probe 27: four, word must NOT match four word
        assert_eq!(
            apply_corrections("Look at four, word here.", &rules),
            "Look at four, word here."
        );
        // But four word matches
        assert_eq!(
            apply_corrections("Look at four word here.", &rules),
            "Look at forward here."
        );
    }
}
