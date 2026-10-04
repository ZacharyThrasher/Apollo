//! Cleaning track titles and artist names so a library entry and a lyrics database entry for
//! the same song compare equal: `World's Greatest!` / `Worlds Greatest`, `Big K.R.I.T.` /
//! `Big KRIT`, `Halo - Remastered 2009` / `Halo`, `Sigur Rós` / `Sigur Ros`.

/// A bracket group or ` - …` tail starting with one of these is a decoration of the title rather
/// than part of it. "remix", "live" and "acoustic" are absent: those are other recordings.
const DECORATION_STARTERS: &[&str] = &[
    "feat",
    "ft",
    "featuring",
    "from",
    "remastered",
    "remaster",
    "bonus",
    "explicit",
    "clean",
    "deluxe",
    "expanded",
    "official",
    "original",
    "mono",
    "stereo",
    "radio",
    "single",
    "album",
];

/// Anywhere in the group, one of these marks it too (`2011 Remaster`, `Single Version`).
const DECORATION_ANYWHERE: &[&str] = &["remaster", "remastered", "version", "edit", "edition"];

/// One character folded to its plain Latin letter(s); other characters pass through.
fn fold_char(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'ß' => "ss",
        'æ' => "ae",
        'œ' => "oe",
        _ => return None,
    })
}

/// A comparison key: lowercase letters and digits only, with diacritics folded. Punctuation and
/// spacing never decide whether two names are the same (`K.R.I.T.` = `KRIT`, `AC/DC` = `AC DC`).
pub fn key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match fold_char(c) {
            Some(folded) => out.push_str(folded),
            None if c.is_alphanumeric() => out.push(c),
            None => {}
        }
    }
    out
}

fn is_decoration(inner: &str) -> bool {
    let lower = inner.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words
        .first()
        .is_some_and(|w| DECORATION_STARTERS.contains(w))
        || words.iter().any(|w| DECORATION_ANYWHERE.contains(w))
}

/// The title as the song itself is called: trailing `(feat. …)`, `[Remastered]`,
/// ` - 2011 Remaster`, ` - Radio Edit` and ` feat. …` removed, and trailing `!`/`.` dropped.
/// Brackets that are part of the name stay: `(I Can't Get No) Satisfaction`, `Song (Pt. 2)`.
/// Never returns an empty string for a non-empty title.
pub fn clean_title(title: &str) -> String {
    let mut t = title.trim().to_owned();
    loop {
        let before = t.len();
        t = t.trim_end().to_owned();
        // A trailing bracket group that is a decoration.
        if let Some(close) = t.chars().last().filter(|c| matches!(c, ')' | ']' | '}')) {
            let open = match close {
                ')' => '(',
                ']' => '[',
                _ => '{',
            };
            if let Some(start) = t.rfind(open) {
                if start > 0 && is_decoration(&t[start + 1..t.len() - 1]) {
                    t.truncate(start);
                }
            }
        }
        // A ` - tail` that is a decoration.
        for dash in [" - ", " – ", " — "] {
            if let Some(at) = t.rfind(dash) {
                if at > 0 && is_decoration(&t[at + dash.len()..]) {
                    t.truncate(at);
                }
            }
        }
        // An unbracketed ` feat. X` / ` ft. X` / ` featuring X` tail.
        let lower = t.to_lowercase();
        for marker in [" feat.", " feat ", " ft.", " ft ", " featuring "] {
            if let Some(at) = lower.find(marker) {
                if at > 0 && lower.len() == t.len() && t.is_char_boundary(at) {
                    t.truncate(at);
                    break;
                }
            }
        }
        if t.len() == before {
            break;
        }
    }
    let t = t.trim().trim_end_matches(['!', '.', ',', '…']).trim_end();
    if t.is_empty() {
        title.trim().to_owned()
    } else {
        t.to_owned()
    }
}

/// The individual names in an artist field: `A, B`, `A & B`, `A feat. B`, `A x B`, `A; B`, plus
/// the whole field. Each is a [`key`], and a leading "the" is ignored.
pub fn artist_keys(field: &str) -> Vec<String> {
    let mut names = vec![field.to_owned()];
    let mut parts = vec![field.to_owned()];
    for sep in [
        ",",
        ";",
        " & ",
        " and ",
        " x ",
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " with ",
        " / ",
    ] {
        parts = parts
            .iter()
            .flat_map(|p| p.split(sep).map(str::to_owned).collect::<Vec<_>>())
            .collect();
    }
    names.extend(parts);
    let mut keys: Vec<String> = names
        .iter()
        .map(|n| {
            let k = key(n);
            k.strip_prefix("the")
                .filter(|rest| rest.len() >= 3 && n.trim().to_lowercase().starts_with("the "))
                .map_or(k.clone(), str::to_owned)
        })
        .filter(|k| !k.is_empty())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Whether two artist fields share a name.
pub fn same_artist(a: &str, b: &str) -> bool {
    let b_keys = artist_keys(b);
    artist_keys(a).iter().any(|k| b_keys.contains(k))
}

/// A search result's title, with a leading `Artist - ` (uploads named like videos) removed when
/// the artist is one of `artists`.
pub fn strip_artist_prefix(title: &str, artists: &str) -> String {
    for dash in [" - ", " – ", " — "] {
        if let Some((head, rest)) = title.split_once(dash) {
            if same_artist(head, artists) && !rest.trim().is_empty() {
                return rest.trim().to_owned();
            }
        }
    }
    title.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_case_punctuation_and_diacritics() {
        assert_eq!(key("World's Greatest!"), key("Worlds greatest"));
        assert_eq!(key("World’s Greatest"), key("World's Greatest"));
        assert_eq!(key("Big K.R.I.T."), key("Big KRIT"));
        assert_eq!(key("Sigur Rós"), key("Sigur Ros"));
        assert_eq!(key("Beyoncé"), "beyonce");
        assert_eq!(key("AC/DC"), key("AC DC"));
        assert_ne!(key("Halo"), key("Hello"));
    }

    #[test]
    fn decorations_come_off_the_end_of_a_title() {
        for (given, want) in [
            ("Starboy (feat. Daft Punk)", "Starboy"),
            ("Starboy (Feat. Daft Punk) [Explicit]", "Starboy"),
            ("Halo - Remastered 2009", "Halo"),
            ("Come Together - 2019 Remaster", "Come Together"),
            ("Blinding Lights - Radio Edit", "Blinding Lights"),
            ("Song (Single Version)", "Song"),
            ("Song feat. Someone", "Song"),
            ("Song ft. Someone Else", "Song"),
            ("Song - From \"The Movie\"", "Song"),
            ("World's Greatest!", "World's Greatest"),
            ("Song (Remastered) (feat. X)", "Song"),
            ("Song [Deluxe Edition]", "Song"),
        ] {
            assert_eq!(clean_title(given), want, "{given}");
        }
    }

    #[test]
    fn brackets_that_are_part_of_the_name_stay() {
        for title in [
            "(I Can't Get No) Satisfaction",
            "Song (Pt. 2)",
            "Song (Remix)",
            "Song (Live)",
            "Song (Acoustic)",
            "Hello - Part 2",
            "(feat. X)",
            "Heroes - Live",
        ] {
            assert_eq!(clean_title(title), title);
        }
        assert_eq!(clean_title("Wow!"), "Wow");
        assert_eq!(clean_title("!!!"), "!!!");
    }

    #[test]
    fn an_artist_field_is_a_set_of_names() {
        assert!(same_artist("The Weeknd", "Weeknd"));
        assert!(same_artist("The Weeknd, Daft Punk", "Daft Punk"));
        assert!(same_artist("Big K.R.I.T.", "BIG KRIT"));
        assert!(same_artist("Lil Nas X feat. Jack Harlow", "Lil Nas X"));
        assert!(same_artist("Simon & Garfunkel", "Garfunkel"));
        assert!(!same_artist("Lil Nas X", "Lil Wayne"));
        assert!(!same_artist("Booker T. & the MG's", "Big K.R.I.T."));
    }

    #[test]
    fn an_artist_prefix_is_stripped_only_for_the_right_artist() {
        assert_eq!(
            strip_artist_prefix("The Weeknd - Blinding Lights", "The Weeknd"),
            "Blinding Lights"
        );
        assert_eq!(
            strip_artist_prefix("Halo - Remastered", "Beyoncé"),
            "Halo - Remastered"
        );
    }
}
