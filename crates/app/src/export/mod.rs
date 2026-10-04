//! Settings > Data > Export library: the account's playlists (with their tracks), Liked Music,
//! saved albums and subscribed artists, written to a CSV or JSON file.
//!
//! Read-only by construction: [`collect`] only calls the library source's reads, and the file
//! is the only thing written. This module is the file's shape (pure, no GPUI); `collect` fills it.

pub mod collect;

use std::time::Duration;

use apollo_domain::library::bare_playlist_id;
use serde::Serialize;

use crate::diagnostics::time::iso_utc;
use crate::library::TrackRow;

/// The base of every link in an export.
const MUSIC: &str = "https://music.youtube.com";

/// What the file is written as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportFormat {
    #[default]
    Json,
    Csv,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 2] = [Self::Json, Self::Csv];

    pub fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Csv => "CSV",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }

    /// What the setting stores.
    pub fn stored(self) -> &'static str {
        self.extension()
    }

    /// An unknown or empty value is the default (JSON).
    pub fn from_stored(text: &str) -> Self {
        match text.trim() {
            "csv" => Self::Csv,
            _ => Self::Json,
        }
    }
}

/// One song, as the file says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExportTrack {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_seconds: Option<u64>,
    pub explicit: bool,
    pub url: String,
}

impl From<&TrackRow> for ExportTrack {
    fn from(row: &TrackRow) -> Self {
        let t = &row.track;
        Self {
            video_id: t.video_id.as_str().to_owned(),
            title: t.title.clone(),
            artists: t.artists.iter().map(|a| a.name.clone()).collect(),
            album: t.album.as_ref().map(|a| a.title.clone()),
            duration_seconds: t.duration.map(|d: Duration| d.as_secs()),
            explicit: row.explicit,
            url: format!("{MUSIC}/watch?v={}", t.video_id.as_str()),
        }
    }
}

/// A playlist and its songs, in the order the account has them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExportPlaylist {
    pub id: String,
    pub title: String,
    /// The service's own second line (owner, size), as it gave it.
    pub detail: Option<String>,
    pub url: String,
    pub tracks: Vec<ExportTrack>,
}

impl ExportPlaylist {
    pub fn new(id: &str, title: &str, detail: Option<&str>, tracks: Vec<ExportTrack>) -> Self {
        let bare = bare_playlist_id(id);
        Self {
            id: bare.to_owned(),
            title: title.to_owned(),
            detail: detail.map(str::to_owned),
            url: format!("{MUSIC}/playlist?list={bare}"),
            tracks,
        }
    }
}

/// A saved album or a subscribed artist: the entry, not its songs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExportEntry {
    pub id: String,
    pub title: String,
    pub detail: Option<String>,
    pub url: String,
}

impl ExportEntry {
    pub fn album(id: &str, title: &str, detail: Option<&str>) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            detail: detail.map(str::to_owned),
            url: format!("{MUSIC}/browse/{id}"),
        }
    }

    pub fn artist(id: &str, title: &str, detail: Option<&str>) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            detail: detail.map(str::to_owned),
            url: format!("{MUSIC}/channel/{id}"),
        }
    }
}

/// Something that could not be read, so the file is missing it (or part of it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub what: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryExport {
    pub exported_at: u64,
    pub liked_songs: Vec<ExportTrack>,
    pub playlists: Vec<ExportPlaylist>,
    pub saved_albums: Vec<ExportEntry>,
    pub subscribed_artists: Vec<ExportEntry>,
    pub problems: Vec<Problem>,
}

impl LibraryExport {
    pub fn is_complete(&self) -> bool {
        self.problems.is_empty()
    }
}

/// The JSON file's top level: a fixed `format` and `version` so a later reader can tell what it
/// has, then the sections.
#[derive(Serialize)]
struct JsonFile<'a> {
    format: &'static str,
    version: u32,
    exported_at: String,
    liked_songs: &'a [ExportTrack],
    playlists: &'a [ExportPlaylist],
    saved_albums: &'a [ExportEntry],
    subscribed_artists: &'a [ExportEntry],
    /// Empty when everything was read; otherwise what is missing.
    problems: &'a [Problem],
}

pub fn render(export: &LibraryExport, format: ExportFormat) -> Vec<u8> {
    match format {
        ExportFormat::Json => render_json(export).into_bytes(),
        ExportFormat::Csv => render_csv(export).into_bytes(),
    }
}

pub fn render_json(export: &LibraryExport) -> String {
    let file = JsonFile {
        format: "apollo-library-export",
        version: 1,
        exported_at: iso_utc(export.exported_at),
        liked_songs: &export.liked_songs,
        playlists: &export.playlists,
        saved_albums: &export.saved_albums,
        subscribed_artists: &export.subscribed_artists,
        problems: &export.problems,
    };
    let mut text = serde_json::to_string_pretty(&file).expect("the export serializes");
    text.push('\n');
    text
}

/// The columns of the CSV file, one table for everything: `section` says what a row is
/// (`liked_song`, `playlist_song`, `saved_album`, `subscribed_artist`, or `problem`).
pub const CSV_HEADER: [&str; 12] = [
    "section",
    "list",
    "list_id",
    "position",
    "video_id",
    "title",
    "artists",
    "album",
    "duration_seconds",
    "explicit",
    "detail",
    "url",
];

/// UTF-8 with a byte-order mark (so Excel reads the accents), CRLF line ends, RFC 4180 quoting.
/// A text cell that would run as a formula in a spreadsheet (starts with `=`, `+`, `-`, `@`, a
/// tab or a carriage return) gets a leading apostrophe; the JSON file is the exact one.
pub fn render_csv(export: &LibraryExport) -> String {
    let mut out = String::from('\u{feff}');
    push_row(&mut out, CSV_HEADER.iter().map(|s| s.to_string()));

    let song =
        |out: &mut String, section: &str, list: &str, list_id: &str, n: usize, t: &ExportTrack| {
            push_row(
                out,
                [
                    section.to_owned(),
                    list.to_owned(),
                    list_id.to_owned(),
                    (n + 1).to_string(),
                    t.video_id.clone(),
                    t.title.clone(),
                    t.artists.join("; "),
                    t.album.clone().unwrap_or_default(),
                    t.duration_seconds
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    t.explicit.to_string(),
                    String::new(),
                    t.url.clone(),
                ],
            );
        };
    for (n, t) in export.liked_songs.iter().enumerate() {
        song(&mut out, "liked_song", "Liked Music", "LM", n, t);
    }
    for p in &export.playlists {
        for (n, t) in p.tracks.iter().enumerate() {
            song(&mut out, "playlist_song", &p.title, &p.id, n, t);
        }
    }
    let entry = |out: &mut String, section: &str, e: &ExportEntry| {
        push_row(
            out,
            [
                section.to_owned(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                e.title.clone(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                e.detail.clone().unwrap_or_default(),
                e.url.clone(),
            ],
        );
    };
    for a in &export.saved_albums {
        entry(&mut out, "saved_album", a);
    }
    for a in &export.subscribed_artists {
        entry(&mut out, "subscribed_artist", a);
    }
    for p in &export.problems {
        let mut cells = vec![String::new(); CSV_HEADER.len()];
        cells[0] = "problem".to_owned();
        cells[5] = p.what.clone();
        cells[10] = p.reason.clone();
        push_row(&mut out, cells);
    }
    out
}

fn push_row(out: &mut String, cells: impl IntoIterator<Item = String>) {
    let mut first = true;
    for cell in cells {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&csv_cell(&cell));
    }
    out.push_str("\r\n");
}

fn csv_cell(raw: &str) -> String {
    let text = if raw.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{raw}")
    } else {
        raw.to_owned()
    };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

/// The file name the save dialog proposes: `apollo-library-2026-10-04.json`.
pub fn suggested_name(format: ExportFormat, now_unix: u64) -> String {
    format!(
        "apollo-library-{}.{}",
        crate::diagnostics::time::date_utc(now_unix),
        format.extension()
    )
}

/// Reads the library and writes it to `path`. `Ok(None)` is a complete file; `Ok(Some(text))` is
/// a file that is missing something (what the row says); `Err(text)` wrote nothing. A path with
/// no extension gets the format's.
pub fn export_to_file(
    source: &dyn crate::library::LibrarySource,
    path: &std::path::Path,
    format: ExportFormat,
    now_unix: u64,
    options: collect::Options,
) -> Result<Option<String>, &'static str> {
    let export = collect::collect(source, now_unix, options).map_err(collect::ExportError::text)?;
    write_file(path, format, &render(&export, format))?;
    Ok(export.problem_text())
}

/// `path`, with the format's extension when the person typed none.
pub fn with_extension(path: &std::path::Path, format: ExportFormat) -> std::path::PathBuf {
    match path.extension() {
        Some(_) => path.to_owned(),
        None => path.with_extension(format.extension()),
    }
}

/// Writes through a `.part` file beside the target and renames it over, so an export that dies
/// halfway never leaves a truncated file where a good one was.
pub fn write_file(
    path: &std::path::Path,
    format: ExportFormat,
    bytes: &[u8],
) -> Result<(), &'static str> {
    const FAILED: &str = "Couldn't write the file";
    let path = with_extension(path, format);
    let mut part = path.clone().into_os_string();
    part.push(".part");
    let part = std::path::PathBuf::from(part);
    std::fs::write(&part, bytes)
        .and_then(|()| std::fs::rename(&part, &path))
        .map_err(|_| {
            let _ = std::fs::remove_file(&part);
            FAILED
        })
}

#[cfg(test)]
mod tests;
