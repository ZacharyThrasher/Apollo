//! LRC parsing: `[mm:ss.xx] text` lines, several stamps on one line, the `[offset:]` tag, and
//! the enhanced-LRC word stamps (`<mm:ss.xx>`), which are dropped (lines are what is shown).

use crate::Line;

/// Parse LRC text into lines ordered by time. Lines without a stamp (plain text, tags other
/// than `offset`) and lines with no text are left out; nothing here fails.
pub fn parse_lrc(src: &str) -> Vec<Line> {
    let mut offset_ms: i64 = 0;
    let mut lines = Vec::new();
    for raw in src.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some(end) = tag.find(']') else { break };
            let (inner, after) = (&tag[..end], &tag[end + 1..]);
            if let Some(ms) = parse_stamp(inner) {
                stamps.push(ms);
            } else if let Some(value) = inner.strip_prefix("offset:") {
                offset_ms = value.trim().parse().unwrap_or(0);
            }
            rest = after;
        }
        if stamps.is_empty() {
            continue;
        }
        let text = strip_word_stamps(rest);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        for start_ms in stamps {
            lines.push(Line {
                start_ms,
                text: text.to_owned(),
            });
        }
    }
    // A positive offset makes the lyrics appear sooner (LRC convention).
    for line in &mut lines {
        line.start_ms = line.start_ms.saturating_add_signed(-offset_ms);
    }
    lines.sort_by_key(|l| l.start_ms);
    lines
}

/// `mm:ss`, `mm:ss.f`, `mm:ss.ff`, `mm:ss.fff` (a `:` before the fraction is accepted too).
fn parse_stamp(s: &str) -> Option<u64> {
    let (min, rest) = s.split_once(':')?;
    let (sec, frac) = match rest.find(['.', ':']) {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    if min.is_empty() || !min.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if sec.is_empty() || sec.len() > 2 || !sec.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ms = match frac {
        None => 0,
        Some(f) if !f.is_empty() && f.len() <= 3 && f.bytes().all(|b| b.is_ascii_digit()) => {
            let n: u64 = f.parse().ok()?;
            n * 10u64.pow(3 - f.len() as u32)
        }
        Some(_) => return None,
    };
    let (min, sec): (u64, u64) = (min.parse().ok()?, sec.parse().ok()?);
    (sec < 60).then_some((min * 60 + sec) * 1000 + ms)
}

/// Remove `<mm:ss.xx>` word stamps from a line's text.
fn strip_word_stamps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        match after.find('>') {
            Some(close) if parse_stamp(&after[..close]).is_some() => {
                out.push_str(&rest[..open]);
                rest = &after[close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    // Word stamps sit between words, so removing them can leave doubled spaces.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Write lines back as LRC, to the millisecond. `parse_lrc(to_lrc(l)) == l`.
pub fn to_lrc(lines: &[Line]) -> String {
    let mut out = String::new();
    for line in lines {
        let (secs, ms) = (line.start_ms / 1000, line.start_ms % 1000);
        out.push_str(&format!(
            "[{:02}:{:02}.{:03}]{}\n",
            secs / 60,
            secs % 60,
            ms,
            line.text
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(lines: &[Line]) -> Vec<u64> {
        lines.iter().map(|l| l.start_ms).collect()
    }

    #[test]
    fn stamps_parse_to_milliseconds_at_any_precision() {
        let lines = parse_lrc("[00:13.13] a\n[01:02.5] b\n[00:03.123] c\n[02:00] d\n[00:01:50] e");
        assert_eq!(starts(&lines), vec![1500, 3123, 13130, 62500, 120000]);
        let text: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(text, ["e", "c", "a", "b", "d"]);
    }

    #[test]
    fn one_line_with_several_stamps_repeats() {
        let lines = parse_lrc("[00:10.00][00:30.00] chorus\n[00:20.00] verse");
        assert_eq!(starts(&lines), vec![10000, 20000, 30000]);
        assert_eq!(lines[0].text, "chorus");
        assert_eq!(lines[1].text, "verse");
        assert_eq!(lines[2].text, "chorus");
    }

    #[test]
    fn the_offset_tag_moves_every_line_and_never_goes_below_zero() {
        let lines = parse_lrc("[offset:+500]\n[00:00.20] a\n[00:05.00] b");
        assert_eq!(starts(&lines), vec![0, 4500]);
        let lines = parse_lrc("[offset:-250]\n[00:05.00] b");
        assert_eq!(starts(&lines), vec![5250]);
    }

    #[test]
    fn other_tags_blank_lines_plain_lines_and_garbage_are_skipped() {
        let lines = parse_lrc(
            "[ar:Someone]\n[ti:Song]\n\njust words\n[00:01.00]\n[00:xx.00] no\n[61:61.00] no\n[00:02.00 broken\n[00:03.00] kept",
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "kept");
        assert!(parse_lrc("").is_empty());
    }

    #[test]
    fn enhanced_word_stamps_are_removed_but_other_angle_brackets_stay() {
        let lines = parse_lrc("[00:01.00] <00:01.00>Hello <00:01.40>there\n[00:03.00] a <b> c");
        assert_eq!(lines[0].text, "Hello there");
        assert_eq!(lines[1].text, "a <b> c");
    }

    #[test]
    fn writing_then_parsing_gives_the_same_lines() {
        let lines = parse_lrc("[00:13.13] Yeah\n[00:16.56] ♪\n[100:01.007] long");
        assert_eq!(parse_lrc(&to_lrc(&lines)), lines);
    }
}
