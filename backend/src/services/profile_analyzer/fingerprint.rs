//! SQL fingerprinting and literal redaction.
//!
//! `normalize` replaces string and numeric literals with `?`, collapses `IN (?, ?, ...)` lists,
//! removes comments, lowercases unquoted text and collapses whitespace, so runs of the same
//! statement with different literals share a fingerprint. `redact_literals` keeps the original
//! layout and only replaces literals, for exports.
use sha2::{Digest, Sha256};

enum Mode {
    Normalize,
    Redact,
}

fn scan(sql: &str, mode: Mode) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    let normalize = matches!(mode, Mode::Normalize);
    let prev_is_word = |out: &str| {
        out.chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '`' || c == '#')
    };
    while i < chars.len() {
        let c = chars[i];
        // Comments.
        if c == '-' && chars.get(i + 1) == Some(&'-') || c == '#' && !prev_is_word(&out) {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            if !normalize {
                out.push(' ');
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let hint = chars.get(i + 2) == Some(&'+');
            let start = i;
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            if hint {
                // Optimizer hints change the plan; keep them.
                out.extend(&chars[start..i]);
            } else if !normalize {
                out.push(' ');
            }
            continue;
        }
        // String literals.
        if c == '\'' || c == '"' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if chars[i] == c {
                    if chars.get(i + 1) == Some(&c) {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            out.push('?');
            continue;
        }
        // Quoted identifiers are kept verbatim.
        if c == '`' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '`' {
                i += 1;
            }
            i = (i + 1).min(chars.len());
            out.extend(&chars[start..i]);
            continue;
        }
        // Numbers not attached to an identifier (t1, col_2 stay intact).
        if c.is_ascii_digit() && !prev_is_word(&out) {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            out.push('?');
            continue;
        }
        if normalize {
            if c.is_whitespace() {
                if !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
            } else {
                out.extend(c.to_lowercase());
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    if normalize { collapse_lists(out.trim().trim_end_matches(';').trim()) } else { out }
}

/// `in (?, ?, ?)` and `values (?, ?), (?, ?)` -> a single placeholder group.
fn collapse_lists(sql: &str) -> String {
    let mut out = sql
        .replace("( ?", "(?")
        .replace("? )", "?)")
        .replace(" ,", ",")
        .replace(", ", ",");
    loop {
        let next = out.replace("?,?", "?").replace("(?),(?)", "(?)");
        if next == out {
            return out.replace(',', ", ");
        }
        out = next;
    }
}

pub fn normalize(sql: &str) -> String {
    scan(sql, Mode::Normalize)
}

/// First 16 hex characters of SHA-256 over the normalized statement.
pub fn fingerprint(sql: &str) -> String {
    let digest = Sha256::digest(normalize(sql).as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

pub fn redact_literals(sql: &str) -> String {
    scan(sql, Mode::Redact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_whitespace_case_and_comments_do_not_change_the_fingerprint() {
        let a = "SELECT id FROM t WHERE name = 'alice' AND age > 30 -- note\nLIMIT 10;";
        let b = "select  id\nfrom t /* c */ where NAME = \"bob\" and age > 41 limit 5";
        assert_eq!(normalize(a), "select id from t where name = ? and age > ? limit ?");
        assert_eq!(fingerprint(a), fingerprint(b));
        assert_eq!(fingerprint(a).len(), 16);
    }

    #[test]
    fn in_lists_collapse_and_identifiers_with_digits_survive() {
        assert_eq!(
            normalize("SELECT c1 FROM t2 WHERE x IN (1, 2, 3)"),
            "select c1 from t2 where x in (?)"
        );
        assert_eq!(
            fingerprint("SELECT * FROM t WHERE x IN (1)"),
            fingerprint("SELECT * FROM t WHERE x IN (1,2,3,4)")
        );
        assert_ne!(fingerprint("SELECT a FROM t1"), fingerprint("SELECT a FROM t2"));
    }

    #[test]
    fn quoted_identifiers_and_hints_are_kept() {
        assert_eq!(normalize("SELECT `Order` FROM `db`.`T1`"), "select `Order` from `db`.`T1`");
        assert!(normalize("SELECT /*+ SET_VAR(a=1) */ 1").contains("/*+ SET_VAR(a=1) */"));
    }

    #[test]
    fn redaction_keeps_layout_but_removes_values() {
        let sql = "SELECT id\nFROM t WHERE email = 'x@y.z' AND id = 42 AND note = 'it''s'";
        assert_eq!(
            redact_literals(sql),
            "SELECT id\nFROM t WHERE email = ? AND id = ? AND note = ?"
        );
        assert_eq!(redact_literals("x = 'unterminated"), "x = ?");
    }
}
