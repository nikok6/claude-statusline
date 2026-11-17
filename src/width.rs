use unicode_width::UnicodeWidthChar;

/// Terminal columns from `$COLUMNS`. Claude Code sets it before running the
/// statusline because the script's stdout is a pipe, not a TTY.
pub fn terminal_columns() -> Option<usize> {
    std::env::var("COLUMNS").ok()?.trim().parse().ok().filter(|&c| c > 0)
}

/// Display width in terminal cells, ignoring the SGR (`\x1b[...m`) and
/// OSC 8 hyperlink (`\x1b]...\x1b\\`) sequences this binary emits.
pub fn visible_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            width += c.width().unwrap_or(0);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() { break; }
                }
            }
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\x07' { break; }
                    if next == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    width
}

fn plain_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Shortens plain text to at most `max` cells by replacing the middle with
/// `…`, keeping the start and end (`fix/unlinked-auth-clearing-reversal` →
/// `fix/unlin…reversal`). Returns the text unchanged if it already fits.
pub fn truncate_middle(s: &str, max: usize) -> String {
    if plain_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max - 1; // one cell for the ellipsis
    let head_budget = budget.div_ceil(2);
    let tail_budget = budget - head_budget;

    let mut head = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > head_budget { break; }
        used += w;
        head.push(c);
    }
    // A joiner whose next emoji didn't fit would dangle before the ellipsis.
    while head.ends_with('\u{200D}') {
        head.pop();
    }

    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for c in s.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > tail_budget { break; }
        used += w;
        tail.push(c);
    }
    // Zero-width chars (combining accents, joiners, variation selectors) always
    // fit the budget, so one can be kept while its base char was cut; it would
    // then attach to the ellipsis (`…́`). `tail` is reversed, so its last char
    // is the leftmost.
    while tail.last().is_some_and(|c| c.width() == Some(0)) {
        tail.pop();
    }
    tail.reverse();

    format!("{}…{}", head, tail.into_iter().collect::<String>())
}

/// Picks display widths for dir and branch so the line fits. `overflow` is how
/// many cells too wide the line is. Shrinks whichever is currently longer, one
/// cell at a time, never below `min` (or the original width if shorter).
/// Returns the target widths; they may still overflow if both hit `min`.
pub fn fit_two(dir: &str, branch: &str, mut overflow: usize, min: usize) -> (usize, usize) {
    let (mut d, mut b) = (plain_width(dir), plain_width(branch));
    let (d_floor, b_floor) = (d.min(min), b.min(min));
    while overflow > 0 {
        let shrink_dir = if d > d_floor && b > b_floor { d >= b } else { d > d_floor };
        if shrink_dir {
            d -= 1;
        } else if b > b_floor {
            b -= 1;
        } else {
            break;
        }
        overflow -= 1;
    }
    (d, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_ignores_color_and_hyperlink_escapes() {
        let s = "\x1b[38;2;1;2;3m\x1b]8;;https://x.y/z\x1b\\repo\x1b]8;;\x1b\\\x1b[0m | main";
        assert_eq!(visible_width(s), "repo | main".len());
    }

    #[test]
    fn width_counts_wide_chars_as_two() {
        assert_eq!(visible_width("a🚀b"), 4);
        assert_eq!(visible_width("…▓░"), 3);
    }

    #[test]
    fn truncate_keeps_head_and_tail() {
        let out = truncate_middle("fix/unlinked-auth-clearing-reversal", 18);
        assert_eq!(out, "fix/unlin…reversal");
        assert_eq!(plain_width(&out), 18);
    }

    #[test]
    fn truncate_noop_when_fits() {
        assert_eq!(truncate_middle("main", 10), "main");
    }

    #[test]
    fn truncate_never_splits_wide_char_over_budget() {
        let out = truncate_middle("🚀🚀🚀🚀🚀🚀", 6);
        assert!(plain_width(&out) <= 6, "{out}");
    }

    #[test]
    fn truncate_drops_orphaned_combining_mark() {
        // decomposed é = 'e' + U+0301; cut lands between them
        let out = truncate_middle("xxxxxxxxxxxxxxxxxe\u{301}abcd", 10);
        assert!(!out.contains("…\u{301}"), "{out:?}");
        assert_eq!(out, "xxxxx…abcd");
    }

    #[test]
    fn truncate_drops_dangling_joiner() {
        let family = "👨\u{200D}👩\u{200D}👧";
        let out = truncate_middle(&format!("{}abc", family.repeat(6)), 8);
        assert!(!out.contains("\u{200D}…"), "{out:?}");
    }

    #[test]
    fn fit_shrinks_longer_first() {
        // dir 29, branch 35, need 9 off → branch alone down to 29, then alternate
        assert_eq!(fit_two(&"d".repeat(29), &"b".repeat(35), 9, 10), (27, 28));
        // need 4 → branch only
        assert_eq!(fit_two(&"d".repeat(29), &"b".repeat(35), 4, 10), (29, 31));
        // need 12 → branch down to 29, then alternate
        assert_eq!(fit_two(&"d".repeat(29), &"b".repeat(35), 12, 10), (26, 26));
    }

    #[test]
    fn fit_respects_floor() {
        assert_eq!(fit_two(&"d".repeat(15), &"b".repeat(4), 100, 10), (10, 4));
    }
}
