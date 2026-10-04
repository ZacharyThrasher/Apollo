//! YouTube Music's own lyrics: the plain text on a lyrics tab (`MPLYt…` browse page).
//!
//! The page is a `musicDescriptionShelfRenderer` whose description runs are the lyrics. Timed
//! lyrics need another client and are not read. A track with no lyrics has no tab at all, or a
//! tab whose page has no description shelf; both read as `None`.

use serde_json::Value;

use super::metadata::find_all;

/// The lyrics text of a lyrics-tab `browse` response, trimmed; `None` when it holds none.
pub fn parse_lyrics_page(root: &Value) -> Option<String> {
    let shelf = find_all(root, "musicDescriptionShelfRenderer").into_iter().next()?;
    let runs = shelf.pointer("/description/runs")?.as_array()?;
    let text: String =
        runs.iter().filter_map(|run| run.get("text").and_then(Value::as_str)).collect();
    let text = text.replace("\r\n", "\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_description_runs_as_one_text() {
        let root = json!({
            "contents": { "sectionListRenderer": { "contents": [
                { "musicDescriptionShelfRenderer": {
                    "header": { "runs": [{ "text": "Lyrics" }] },
                    "description": { "runs": [{ "text": "First line\nSecond line\r\n\nChorus\n" }] },
                    "footer": { "runs": [{ "text": "Source: Musixmatch" }] }
                } }
            ] } }
        });
        assert_eq!(parse_lyrics_page(&root).as_deref(), Some("First line\nSecond line\n\nChorus"));
    }

    #[test]
    fn a_page_without_a_description_or_with_a_blank_one_has_no_lyrics() {
        let message = json!({
            "contents": { "sectionListRenderer": { "contents": [
                { "messageRenderer": { "text": { "runs": [{ "text": "Lyrics not available" }] } } }
            ] } }
        });
        assert_eq!(parse_lyrics_page(&message), None);
        let blank = json!({ "musicDescriptionShelfRenderer": {
            "description": { "runs": [{ "text": "  \n " }] } } });
        assert_eq!(parse_lyrics_page(&blank), None);
        assert_eq!(parse_lyrics_page(&json!({})), None);
    }
}
