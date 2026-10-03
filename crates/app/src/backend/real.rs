//! Real services (Windows, cargo feature `real-services`): the M2 account session, the pinned
//! yt-dlp resolver and the libmpv engine behind the runtime ports. The starting queue is the
//! account's Liked Music, listed the way the M2 probe does (no sign-in UI in this slice).
//!
//! Run from the repository root (the pinned tools live under `tools/`):
//!
//! ```text
//! apollo --real [--source auto|stored|firefox|login|file:<path>] [--tracks N] [--silent]
//!               [--tools <dir>] [--select-identity K]
//! ```
//!
//! `--silent` routes audio to mpv's null output, for screenshot runs. Nothing here prints
//! cookies, URLs, titles, ids or account names.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use apollo_account::identity::{InnerTubeValidator, UreqHttp};
use apollo_account::import;
use apollo_account::jar::CookieJar;
use apollo_account::login::{run_login, LoginCancel, LoginConfig};
use apollo_account::service::{AccountService, IdentityState, ImportOutcome, RestoreOutcome};
use apollo_account::store::EncryptedStore;
use apollo_account::vault::{Vault, WindowsCredentialVault};
use apollo_domain::model::{ArtistRef, Track, VideoId};
use apollo_media::{MediaConfig, MpvEngine};
use apollo_resolver::diag::list_liked_music;
use apollo_resolver::{ResolverConfig, YtDlpResolver};
use apollo_services_common::{EventSink, LeaseStore};

use super::real_search::AccountSearch;
use super::{LibraryTrack, Ports, Prepared};
use crate::art::{ArtSource, DecodedArt};
use crate::player::DEFAULT_VOLUME;
use crate::thumbs::ThumbSource;

/// Windows Credential Manager service name for the app's vault key.
const VAULT_SERVICE: &str = "Apollo";
const ART_TIMEOUT: Duration = Duration::from_secs(10);
const ART_MAX_BYTES: u64 = 8 * 1024 * 1024;
const REVALIDATE_POLL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The stored session if it is still valid, else the default Firefox profile.
    Auto,
    Stored,
    Firefox,
    Login,
    File(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub source: Source,
    pub tracks: usize,
    pub silent: bool,
    pub tools: PathBuf,
    pub select_identity: Option<usize>,
}

impl Options {
    pub fn from_args(args: &[String]) -> Result<Self, String> {
        let value = |name: &str| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let source = match value("--source").as_deref() {
            None | Some("auto") => Source::Auto,
            Some("stored") => Source::Stored,
            Some("firefox") => Source::Firefox,
            Some("login") => Source::Login,
            Some(s) if s.starts_with("file:") => Source::File(PathBuf::from(&s[5..])),
            Some(other) => return Err(format!("unknown --source {other}")),
        };
        let number = |name: &str| -> Result<Option<usize>, String> {
            value(name)
                .map(|v| {
                    v.parse::<usize>()
                        .map_err(|_| format!("{name} needs a number"))
                })
                .transpose()
        };
        Ok(Self {
            source,
            tracks: number("--tracks")?.unwrap_or(25).clamp(1, 300),
            silent: args.iter().any(|a| a == "--silent"),
            tools: value("--tools").map_or_else(
                || std::env::current_dir().unwrap_or_default().join("tools"),
                PathBuf::from,
            ),
            select_identity: number("--select-identity")?,
        })
    }
}

pub fn prepare(opts: Options, sink: EventSink) -> Result<Prepared, String> {
    let app_dir = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?)
        .join("Apollo")
        .join("app");
    let account = open_account(&opts, &app_dir)?;
    spawn_revalidation(account.clone());

    let run_dir = app_dir.join("run");
    let rcfg = ResolverConfig::production(&opts.tools, run_dir);
    let leases = LeaseStore::new();
    let resolver = YtDlpResolver::new(
        rcfg.clone(),
        leases.clone(),
        sink.clone(),
        Arc::new(account.clone()),
    )
    .map_err(|e| e.to_string())?;
    let engine = MpvEngine::new(
        MediaConfig {
            audio_output: opts.silent.then(|| "null".to_string()),
            volume: DEFAULT_VOLUME,
            ..Default::default()
        },
        leases,
        sink,
    )
    .map_err(|e| e.to_string())?;

    let listed = list_liked_music(&rcfg, &account, opts.tracks)
        .map_err(|f| format!("Liked Music listing failed: {f:?}"))?;
    let mut queue = Vec::new();
    let mut thumbs = HashMap::new();
    for item in listed {
        let Some(video_id) = VideoId::parse(&item.id) else {
            continue;
        };
        let mut track = Track::new(item.id.clone(), item.title.clone().unwrap_or_default());
        track.video_id = video_id;
        track.artists = item
            .artist
            .iter()
            .map(|name| ArtistRef {
                name: name.clone(),
                id: None,
            })
            .collect();
        track.duration = item.duration_secs.map(Duration::from_secs);
        if let Some(url) = item.thumbnail {
            thumbs.insert(item.id, url);
        }
        queue.push(LibraryTrack { track });
    }
    if queue.is_empty() {
        return Err("Liked Music has no playable tracks".into());
    }

    let search = AccountSearch::new(account.clone())?;
    let art = Arc::new(HttpArt::new(thumbs));

    Ok(Prepared {
        label: "real services",
        ports: Ports {
            resolver: Box::new(resolver),
            engine: Box::new(engine),
        },
        queue,
        art: art.clone(),
        search: Arc::new(search),
        thumbs: art,
    })
}

fn open_account(opts: &Options, app_dir: &std::path::Path) -> Result<AccountService, String> {
    let vault = Arc::new(WindowsCredentialVault::new(VAULT_SERVICE));
    let store = EncryptedStore::new(vault as Arc<dyn Vault>, &app_dir.join("store"))
        .map_err(|e| format!("{e:?}"))?;
    let account = AccountService::new(
        Arc::new(InnerTubeValidator::new(Arc::new(UreqHttp::default()))),
        Some(store),
    );

    let restored = account.restore();
    let stored_ok =
        matches!(restored, RestoreOutcome::Restored) && account.view().can_use_content();
    match (&opts.source, stored_ok) {
        (Source::Stored, true) | (Source::Auto, true) => {}
        (Source::Stored, false) => return Err("no valid stored session".into()),
        (source, _) => {
            let jar = obtain_jar(source, app_dir)?;
            match account.import(jar, true) {
                ImportOutcome::Valid | ImportOutcome::NeedsIdentityChoice => {}
                other => return Err(format!("session rejected: {other:?}")),
            }
            if let IdentityState::Ambiguous(list) = account.view().identity {
                let chosen = opts
                    .select_identity
                    .and_then(|k| list.get(k))
                    .ok_or_else(|| {
                        format!(
                            "the login can act as {} channels; pass --select-identity 0..{}",
                            list.len(),
                            list.len() - 1
                        )
                    })?;
                account.select_identity(&chosen.key());
            }
        }
    }
    if !account.view().can_use_content() {
        return Err("session is not valid".into());
    }
    Ok(account)
}

fn obtain_jar(source: &Source, app_dir: &std::path::Path) -> Result<CookieJar, String> {
    match source {
        Source::Auto | Source::Firefox => {
            let firefox = PathBuf::from(std::env::var_os("APPDATA").ok_or("APPDATA is not set")?)
                .join("Mozilla")
                .join("Firefox");
            let profiles = import::firefox_profiles(&firefox);
            let profile = profiles
                .iter()
                .find(|p| p.is_default)
                .or(profiles.first())
                .ok_or("no Firefox profile")?;
            import::firefox_profile(&profile.path).map_err(|e| e.to_string())
        }
        Source::Login => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let helper = exe.with_file_name("apollo-login-helper.exe");
            run_login(
                &LoginConfig::new(helper, app_dir.join("login")),
                &LoginCancel::new(),
            )
            .map_err(|e| e.to_string())
        }
        Source::File(path) => import::netscape_file(path).map_err(|e| e.to_string()),
        Source::Stored => Err("no valid stored session".into()),
    }
}

/// Keeps the session fresh while the app runs: the account model asks for a re-check about
/// every 30 minutes, and this thread is what actually makes the call (never on the UI thread).
fn spawn_revalidation(account: AccountService) {
    let _ = std::thread::Builder::new()
        .name("apollo-revalidate".into())
        .spawn(move || loop {
            std::thread::sleep(REVALIDATE_POLL);
            if account.needs_revalidation() {
                account.revalidate();
            }
        });
}

/// Artwork from the listing's own thumbnail URL, falling back to the video's standard
/// thumbnail. Plain unauthenticated GETs of image URLs; nothing here touches the session.
struct HttpArt {
    thumbs: HashMap<String, String>,
    agent: ureq::Agent,
}

impl HttpArt {
    fn new(thumbs: HashMap<String, String>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(ART_TIMEOUT))
            .build()
            .into();
        Self { thumbs, agent }
    }

    fn get(&self, url: &str) -> Option<DecodedArt> {
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

impl ArtSource for HttpArt {
    fn fetch(&self, video_id: &str) -> Option<DecodedArt> {
        self.thumbs
            .get(video_id)
            .and_then(|url| self.get(url))
            .or_else(|| self.get(&format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")))
    }
}

/// List rows ask by URL alone; the same plain GET serves them.
impl ThumbSource for HttpArt {
    fn fetch(&self, url: &str) -> Option<DecodedArt> {
        self.get(url)
    }
}
