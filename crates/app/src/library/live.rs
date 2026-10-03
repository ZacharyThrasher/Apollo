//! The library source backed by the real account: InnerTube (through `apollo-provider`) for the
//! lists and track pages, plain HTTP for cover art.
//!
//! Credentials stay with the account service. This adapter asks it for a snapshot, builds a
//! signed-in InnerTube session from the cookies, and hands rotated cookies back (the service
//! rotates `__Secure-*PSIDTS` on these very requests and invalidates the old value, so dropping
//! the new one would eventually sign the account out). Nothing here logs cookies, ids or titles.

use std::sync::Arc;
use std::time::Duration;

use apollo_domain::library::Collection;
use apollo_domain::listing::{LoadFailure, Page};
use apollo_provider::{InnerTubeSource, ListedTrack};
use apollo_services_common::{unix_now, SecretString, SessionProvider};
use innertube::{Locale, Session};

use super::source::{LibrarySource, TrackRow};
use crate::art::DecodedArt;

const ART_TIMEOUT: Duration = Duration::from_secs(10);
const ART_MAX_BYTES: u64 = 8 * 1024 * 1024;

pub struct InnerTubeLibrary {
    rt: tokio::runtime::Runtime,
    source: InnerTubeSource,
    agent: ureq::Agent,
}

impl InnerTubeLibrary {
    /// `data_sync_id` addresses the selected channel when the login can act as several; `None`
    /// for a single-account login.
    pub fn new(
        session: Arc<dyn SessionProvider>,
        data_sync_id: Option<String>,
    ) -> Result<Self, String> {
        let snapshot = session
            .snapshot()
            .ok_or_else(|| "no validated account session".to_string())?;
        let cookie = cookie_header(snapshot.cookies_netscape.expose(), unix_now())
            .ok_or_else(|| "the session has no YouTube cookies".to_string())?;
        let sent = cookie.clone();
        let source = InnerTubeSource::new(
            Session {
                locale: Locale::default(),
                visitor_data: None,
                data_sync_id,
                cookie: Some(cookie),
            },
            None,
        )
        .map_err(|e| e.to_string())?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("apollo-library")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;

        // Rotated cookies go back to the account service, as long as the session it was built
        // from is still current.
        let rotated = source.cookie_changed();
        let reader = source.clone();
        let epoch = snapshot.epoch;
        rt.spawn(async move {
            loop {
                rotated.notified().await;
                let (Some(header), Some(now)) = (reader.cookie(), session.snapshot()) else {
                    continue;
                };
                if now.epoch != epoch {
                    break;
                }
                let merged =
                    merge_header_into_netscape(now.cookies_netscape.expose(), &sent, &header);
                if !session.offer_rotation(epoch, SecretString::new(merged)) {
                    break;
                }
            }
        });

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(ART_TIMEOUT))
            .build()
            .into();
        Ok(Self { rt, source, agent })
    }
}

fn row(listed: ListedTrack) -> TrackRow {
    TrackRow {
        track: listed.track,
        occurrence: listed.occurrence,
        thumbnail: listed.thumbnail,
        explicit: listed.explicit,
    }
}

fn rows(page: Page<ListedTrack>) -> Page<TrackRow> {
    Page {
        items: page.items.into_iter().map(row).collect(),
        continuation: page.continuation,
        skipped: page.skipped,
    }
}

impl LibrarySource for InnerTubeLibrary {
    fn collections(&self) -> Result<Vec<Collection>, LoadFailure> {
        self.rt.block_on(async {
            self.source.ensure_visitor_data().await;
            let mut all = self.source.library_playlists().await?;
            all.extend(self.source.library_albums().await?);
            Ok(all)
        })
    }

    fn tracks(&self, collection: &Collection) -> Result<Page<TrackRow>, LoadFailure> {
        use apollo_domain::library::CollectionKind;
        self.rt.block_on(async {
            self.source.ensure_visitor_data().await;
            let page = match collection.kind {
                CollectionKind::Playlist => self.source.playlist(collection.id.as_str()).await,
                CollectionKind::Album => self.source.album(collection.id.as_str()).await,
            }?;
            Ok(rows(page))
        })
    }

    fn more_tracks(&self, token: &str) -> Result<Page<TrackRow>, LoadFailure> {
        self.rt
            .block_on(async { Ok(rows(self.source.continuation(token).await?)) })
    }

    fn artwork(&self, url: &str) -> Option<DecodedArt> {
        // Cover art is public: an unauthenticated GET, never carrying the session.
        if !url.starts_with("https://") {
            return None;
        }
        let mut resp = self.agent.get(url).call().ok()?;
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(ART_MAX_BYTES)
            .read_to_vec()
            .ok()?;
        DecodedArt::from_encoded(&bytes)
    }
}

// ---- cookies ---------------------------------------------------------------------------------

/// One Netscape line: `domain, include-subdomains, path, secure, expiry, name, value`, with the
/// optional `#HttpOnly_` prefix on the domain.
struct Line<'a> {
    domain: &'a str,
    expiry: i64,
    name: &'a str,
    value: &'a str,
}

fn parse_line(raw: &str) -> Option<Line<'_>> {
    let line = raw.strip_prefix("#HttpOnly_").unwrap_or(raw);
    if line.trim().is_empty() || line.starts_with('#') {
        return None;
    }
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 7 {
        return None;
    }
    Some(Line {
        domain: f[0],
        expiry: f[4].trim().parse().unwrap_or(0),
        name: f[5],
        value: f[6].trim_end_matches('\r'),
    })
}

fn is_youtube(domain: &str) -> bool {
    let d = domain.trim_start_matches('.');
    d == "youtube.com" || d.ends_with(".youtube.com")
}

/// A `Cookie:` header for requests to `music.youtube.com`: the unexpired YouTube-domain cookies
/// of the Netscape text, the more specific domain winning when a name appears twice. `None`
/// when there are none.
pub fn cookie_header(netscape: &str, now: u64) -> Option<String> {
    let mut pairs: Vec<(&str, &str, usize)> = Vec::new();
    for line in netscape.lines().filter_map(parse_line) {
        let alive = line.expiry <= 0 || line.expiry as u64 > now;
        if !alive || !is_youtube(line.domain) {
            continue;
        }
        let specificity = line.domain.len();
        match pairs.iter_mut().find(|(n, _, _)| *n == line.name) {
            Some(existing) if existing.2 >= specificity => {}
            Some(existing) => *existing = (line.name, line.value, specificity),
            None => pairs.push((line.name, line.value, specificity)),
        }
    }
    if pairs.is_empty() {
        return None;
    }
    Some(
        pairs
            .iter()
            .map(|(n, v, _)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// Netscape text with the cookies that rotated written back. `sent` is the header the session
/// started with and `now` the one it holds after a response rotated cookies: only names whose
/// value differs (or that are new) are touched, so a same-named cookie on a different YouTube
/// subdomain keeps its own value. A rotated name replaces the value on each YouTube-domain line
/// of that name; a new name is appended as a session cookie. Everything else is kept as it was.
pub fn merge_header_into_netscape(netscape: &str, sent: &str, now: &str) -> String {
    fn split(h: &str) -> Vec<(&str, &str)> {
        h.split(';')
            .filter_map(|p| p.trim().split_once('='))
            .collect()
    }
    let sent = split(sent);
    let rotated: Vec<(&str, &str)> = split(now)
        .into_iter()
        .filter(|(n, v)| {
            sent.iter()
                .find(|(sn, _)| sn == n)
                .is_none_or(|(_, sv)| sv != v)
        })
        .collect();
    let mut seen = vec![false; rotated.len()];
    let mut out = String::with_capacity(netscape.len() + 64);
    for raw in netscape.lines() {
        let hit = parse_line(raw)
            .filter(|l| is_youtube(l.domain))
            .and_then(|l| rotated.iter().position(|(n, _)| *n == l.name));
        match hit {
            Some(i) => {
                seen[i] = true;
                let (head, _) = raw.rsplit_once('\t').expect("a parsed line has tabs");
                out.push_str(head);
                out.push('\t');
                out.push_str(rotated[i].1);
            }
            None => out.push_str(raw),
        }
        out.push('\n');
    }
    for ((name, value), seen) in rotated.iter().zip(seen) {
        if !seen {
            out.push_str(&format!(
                ".youtube.com\tTRUE\t/\tTRUE\t0\t{name}\t{value}\n"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETSCAPE: &str = "# Netscape HTTP Cookie File\n\
        #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t4102444800\tSID\tsid-value\n\
        .youtube.com\tTRUE\t/\tTRUE\t4102444800\tSAPISID\tsapisid-value\n\
        .google.com\tTRUE\t/\tTRUE\t4102444800\tNID\tgoogle-only\n\
        .youtube.com\tTRUE\t/\tTRUE\t1\tOLD\texpired\n\
        music.youtube.com\tFALSE\t/\tTRUE\t0\tSID\tmusic-sid\n";

    #[test]
    fn the_header_keeps_live_youtube_cookies_and_prefers_the_specific_domain() {
        let header = cookie_header(NETSCAPE, 1_000_000).unwrap();
        let mut parts: Vec<&str> = header.split("; ").collect();
        parts.sort();
        assert_eq!(parts, ["SAPISID=sapisid-value", "SID=music-sid"]);
    }

    #[test]
    fn no_youtube_cookies_means_no_header() {
        assert_eq!(cookie_header("# nothing\n", 0), None);
        assert_eq!(
            cookie_header(".google.com\tTRUE\t/\tTRUE\t0\tNID\tx\n", 0),
            None
        );
    }

    #[test]
    fn rotated_values_replace_in_place_and_new_names_are_appended() {
        let sent = cookie_header(NETSCAPE, 1_000_000).unwrap();
        let now = sent.replace("sapisid-value", "sapisid-rotated") + "; __Secure-1PSIDTS=fresh";
        let merged = merge_header_into_netscape(NETSCAPE, &sent, &now);
        assert!(merged.contains("\tSAPISID\tsapisid-rotated\n"));
        assert!(!merged.contains("sapisid-value"));
        assert!(
            merged.contains("#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t4102444800\tSID\tsid-value\n"),
            "an unrotated cookie keeps its line, flags and expiry (even with a same-named twin)"
        );
        assert!(merged.contains(".google.com\tTRUE\t/\tTRUE\t4102444800\tNID\tgoogle-only\n"));
        assert!(merged.ends_with(".youtube.com\tTRUE\t/\tTRUE\t0\t__Secure-1PSIDTS\tfresh\n"));
    }

    #[test]
    fn a_google_domain_cookie_is_never_overwritten_by_a_youtube_header() {
        let merged = merge_header_into_netscape(NETSCAPE, "SID=x", "NID=from-header");
        assert!(merged.contains("\tNID\tgoogle-only\n"));
        // Appended as a new YouTube-domain cookie instead.
        assert!(merged.ends_with(".youtube.com\tTRUE\t/\tTRUE\t0\tNID\tfrom-header\n"));
    }

    #[test]
    fn an_unchanged_header_changes_nothing() {
        let sent = cookie_header(NETSCAPE, 1_000_000).unwrap();
        assert_eq!(
            merge_header_into_netscape(NETSCAPE, &sent, &sent),
            NETSCAPE.to_string()
        );
    }
}
