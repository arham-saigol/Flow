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
    const OPEN_WRAPPERS: &[char] = &['"', '\'', '(', '[', '{', '“', '‘', '¿', '¡', '«', '‹', '「', '『'];
    const CLOSE_WRAPPERS: &[char] = &['"', '\'', ')', ']', '}', '”', '’', '»', '›', '」', '』'];
    const SENTENCE_FINAL: &[char] = &['.', ',', '!', '?', ';', ':', '…', '。', '！', '？', '؟', '؛'];

    let mut trimmed = collapsed.as_str();

    // Trim outer wrappers and sentence-final punctuation iteratively
    loop {
        let before_len = trimmed.len();
        trimmed = trimmed.trim_start_matches(|c: char| c.is_whitespace() || OPEN_WRAPPERS.contains(&c));
        
        // Strip leading sentence-final punctuation UNLESS it's a '.' followed immediately by an alphanumeric (e.g. .NET)
        let mut chars = trimmed.chars();
        if let Some(first) = chars.next() {
            if SENTENCE_FINAL.contains(&first) {
                let is_dot_identifier = first == '.' && chars.next().map(|c| c.is_alphanumeric()).unwrap_or(false);
                if !is_dot_identifier {
                    trimmed = &trimmed[first.len_utf8()..];
                }
            }
        }

        trimmed = trimmed.trim_end_matches(|c: char| c.is_whitespace() || CLOSE_WRAPPERS.contains(&c) || SENTENCE_FINAL.contains(&c));
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

/// Applies validated dictionary corrections to original text.
/// Uses word continuation boundaries and preserves case/formatting outside the replacement.
pub fn apply_corrections(text: &str, rules: &[ValidatedCorrection]) -> String {
    if text.is_empty() || rules.is_empty() {
        return text.to_string();
    }

    // Build character map of original text:
    // Each char in normalized text corresponds to an original byte span.
    // However, Unicode lowercase/NFC can change character counts and byte lengths!
    // To solve this accurately:
    // We scan the original text, normalizing word windows or finding matching spans.
    
    // We can tokenize or scan `text` using word continuation boundaries.
    // Since corrections are phrases (sequences of words), we extract tokens with their original byte offsets.
    
    #[derive(Debug, Clone)]
    struct Token {
        orig_start: usize,
        orig_end: usize,
        norm: String,
        is_word: bool,
    }

    let mut tokens: Vec<Token> = Vec::new();
    let mut char_indices = text.char_indices().peekable();

    while let Some(&(start, c)) = char_indices.peek() {
        let is_word = is_word_continuation(c);
        let mut end = start + c.len_utf8();
        char_indices.next();

        while let Some(&(next_idx, next_c)) = char_indices.peek() {
            if is_word_continuation(next_c) == is_word {
                end = next_idx + next_c.len_utf8();
                char_indices.next();
            } else {
                break;
            }
        }

        let slice = &text[start..end];
        let norm = if is_word {
            normalize_key(slice)
        } else {
            // For non-word tokens (spaces, punctuation), normalize whitespace
            let nfc: String = slice.nfc().collect();
            nfc.to_lowercase()
        };

        tokens.push(Token {
            orig_start: start,
            orig_end: end,
            norm,
            is_word,
        });
    }

    // Find matches
    // A match spans from token `i` to `j`.
    // It must start and end on a word token (or a token sequence matching normalized_source).
    #[derive(Debug)]
    struct Match {
        orig_start: usize,
        orig_end: usize,
        replacement: String,
        rule_len: usize,
    }

    let mut matches: Vec<Match> = Vec::new();

    for rule in rules {
        let rule_tokens: Vec<&str> = rule.normalized_source.split_whitespace().collect();
        if rule_tokens.is_empty() {
            continue;
        }

        // Try to match rule_tokens starting at token index i
        let mut i = 0;
        while i < tokens.len() {
            // The starting token should be a word token whose norm matches rule_tokens[0]
            if tokens[i].is_word && tokens[i].norm == rule_tokens[0] {
                let mut matched = true;
                let mut r_idx = 1;
                let mut curr_token = i + 1;
                let mut last_match_token = i;

                while r_idx < rule_tokens.len() {
                    // Skip whitespace/punctuation between words if rule has multiple words separated by space
                    while curr_token < tokens.len() && !tokens[curr_token].is_word {
                        curr_token += 1;
                    }
                    if curr_token < tokens.len() && tokens[curr_token].norm == rule_tokens[r_idx] {
                        last_match_token = curr_token;
                        curr_token += 1;
                        r_idx += 1;
                    } else {
                        matched = false;
                        break;
                    }
                }

                if matched && r_idx == rule_tokens.len() {
                    let orig_start = tokens[i].orig_start;
                    let orig_end = tokens[last_match_token].orig_end;

                    // Check boundary: previous char and next char in original text must not be word continuation
                    let left_ok = orig_start == 0 || !text[..orig_start].chars().next_back().map(is_word_continuation).unwrap_or(false);
                    let right_ok = orig_end == text.len() || !text[orig_end..].chars().next().map(is_word_continuation).unwrap_or(false);

                    if left_ok && right_ok {
                        matches.push(Match {
                            orig_start,
                            orig_end,
                            replacement: rule.replacement.clone(),
                            rule_len: rule.normalized_source.len(),
                        });
                    }
                }
            }
            i += 1;
        }
    }

    // Sort matches: left-to-right, then longest rule length first
    matches.sort_by(|a, b| {
        a.orig_start
            .cmp(&b.orig_start)
            .then_with(|| b.rule_len.cmp(&a.rule_len))
            .then_with(|| (b.orig_end - b.orig_start).cmp(&(a.orig_end - a.orig_start)))
    });

    // Select non-overlapping matches left-to-right
    let mut result = String::with_capacity(text.len());
    let mut last_idx = 0;

    for m in matches {
        if m.orig_start < last_idx {
            // Overlaps with an already selected match
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
        assert_eq!(normalize_snippet_trigger("  My   EMAIL address... "), Some("my email address".into()));
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
        assert_eq!(
            apply_corrections("four words", &rules),
            "four words"
        );
        // Underscore is a word continuation: "my_c_project" -> "c" inside identifier should not match
        assert_eq!(
            apply_corrections("my_c_project", &rules),
            "my_c_project"
        );
        // Standalone "c" matches
        assert_eq!(
            apply_corrections("a c project", &rules),
            "a see project"
        );
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
}
