//! Real services (Windows, cargo feature `real-services`): the M2 account session, the pinned
//! yt-dlp resolver and the libmpv engine behind the runtime ports. The starting queue is the
//! account's Liked Music (listed the way the M2 probe does) when someone is signed in; the
//! sign-in screen (Settings > Account) handles everything else.
//!
//! Run from the repository root (the pinned tools live under `tools/`):
//!
//! ```text
//! apollo --real [--source auto|stored|firefox|login|file:<path>] [--tracks N] [--silent]
//!               [--tools <dir>] [--select-identity K]
//! ```
//!
//! `--source` (default `auto`) seeds a session at launch for dev runs; `auto` and `stored` use
//! only the stored session, and the sign-in screen takes over when there is none.
//!
//! `--silent` routes audio to mpv's null output, for screenshot runs. Nothing here prints
//! cookies, URLs, titles, ids or account names.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use apollo_account::identity::{InnerTubeValidator, UreqHttp};
use apollo_account::jar::CookieJar;
use apollo_account::login::{run_login, LoginCancel, LoginConfig, LoginError};
use apollo_account::service::{AccountService, IdentityState, ImportOutcome};
use apollo_account::store::EncryptedStore;
use apollo_account::vault::{Vault, WindowsCredentialVault};
use apollo_domain::model::{ArtistRef, Track, VideoId};
use apollo_media::{MediaConfig, MpvEngine};
use apollo_resolver::diag::list_liked_music;
use apollo_resolver::{ResolverConfig, YtDlpResolver};
use apollo_services_common::{EventSink, LeaseStore};

use super::real_search::AccountSearch;
use super::{LibraryTrack, Ports, Prepared};
use crate::account::{AccountPorts, AvatarSource, SessionSource, SignInError};
use crate::art::{ArtSource, DecodedArt};
use crate::player::DEFAULT_VOLUME;
use crate::url_thumbs::ThumbSource;

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

/// The app's own directory under `%LOCALAPPDATA%`.
fn app_dir() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?)
            .join("Apollo")
            .join("app"),
    )
}

pub fn prepare(
    opts: Options,
    sink: EventSink,
    account: &AccountService,
) -> Result<Prepared, String> {
    let app_dir = app_dir()?;
    let account = account.clone();
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

    // Nothing is listed (and nothing queued) until someone is signed in.
    let listed = if account.view().can_use_content() {
        list_liked_music(&rcfg, &account, opts.tracks)
            .map_err(|f| format!("Liked Music listing failed: {f:?}"))?
    } else {
        Vec::new()
    };
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
    if queue.is_empty() && account.view().can_use_content() {
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

/// The account service over the Windows credential vault, and the WebView2 sign-in helper.
pub fn account_ports(_opts: &Options) -> AccountPorts {
    let source = Arc::new(WindowsSource::new()) as Arc<dyn SessionSource>;
    let validator = Arc::new(InnerTubeValidator::new(Arc::new(UreqHttp::default())));
    let store = app_dir().map_err(|e| e.to_string()).and_then(|dir| {
        let vault = Arc::new(WindowsCredentialVault::new(VAULT_SERVICE));
        EncryptedStore::new(vault as Arc<dyn Vault>, &dir.join("store"))
            .map_err(|e| format!("{e:?}"))
    });
    match store {
        Ok(store) => AccountPorts {
            service: AccountService::new(validator, Some(store)),
            source,
            avatars: Arc::new(HttpAvatars(HttpArt::new(HashMap::new()))),
            startup_error: None,
        },
        Err(err) => {
            eprintln!("account storage unavailable: {err}");
            AccountPorts {
                service: AccountService::new(validator, None),
                source,
                avatars: Arc::new(HttpAvatars(HttpArt::new(HashMap::new()))),
                startup_error: Some(SignInError::Storage),
            }
        }
    }
}

/// Restores the stored session, seeds one from `--source` when asked (dev runs), and starts
/// the periodic re-check. Never fails: when nobody ends up signed in, the sign-in screen is
/// what the owner sees.
pub fn start_account(opts: &Options, ports: &AccountPorts) {
    let account = &ports.service;
    account.restore();
    let seeded = match &opts.source {
        Source::Auto | Source::Stored => None,
        source => Some(obtain_jar(source, &ports.source)),
    };
    if let Some(jar) = seeded {
        match jar.map(|jar| account.import(jar, true)) {
            Ok(ImportOutcome::Valid | ImportOutcome::NeedsIdentityChoice) => {}
            Ok(other) => eprintln!("session rejected: {other:?}"),
            Err(err) => eprintln!("sign-in failed: {}", err.text()),
        }
        if let IdentityState::Ambiguous(list) = account.view().identity {
            match opts.select_identity.and_then(|k| list.get(k)) {
                Some(chosen) => {
                    account.select_identity(&chosen.key());
                }
                None => eprintln!(
                    "the login can act as {} channels; pass --select-identity 0..{}",
                    list.len(),
                    list.len() - 1
                ),
            }
        }
    }
    spawn_revalidation(account.clone());
}

fn obtain_jar(source: &Source, helper: &Arc<dyn SessionSource>) -> Result<CookieJar, SignInError> {
    match source {
        Source::Auto | Source::Firefox => helper.firefox(),
        Source::Login => helper.browser(),
        Source::File(path) => crate::account::file_jar(path),
        Source::Stored => Err(SignInError::NoAuthCookies),
    }
}

/// The sign-in window: the WebView2 helper next to the app's executable.
struct WindowsSource {
    cancel: std::sync::Mutex<LoginCancel>,
}

impl WindowsSource {
    fn new() -> Self {
        Self {
            cancel: std::sync::Mutex::new(LoginCancel::new()),
        }
    }
}

impl SessionSource for WindowsSource {
    fn browser(&self) -> Result<CookieJar, SignInError> {
        let exe = std::env::current_exe().map_err(|_| SignInError::BrowserUnavailable)?;
        let helper = exe.with_file_name("apollo-login-helper.exe");
        let profile_base = app_dir()
            .map_err(|_| SignInError::BrowserUnavailable)?
            .join("login");
        let cancel = LoginCancel::new();
        *self.cancel.lock().unwrap() = cancel.clone();
        run_login(&LoginConfig::new(helper, profile_base), &cancel).map_err(|e| match e {
            LoginError::ClosedByUser | LoginError::Cancelled => SignInError::Cancelled,
            LoginError::TimedOut => SignInError::TimedOut,
            _ => SignInError::BrowserUnavailable,
        })
    }

    fn cancel_browser(&self) {
        self.cancel.lock().unwrap().cancel();
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

/// Channel pictures: plain unauthenticated GETs of the image URL the service named.
struct HttpAvatars(HttpArt);

impl AvatarSource for HttpAvatars {
    fn fetch(&self, url: &str) -> Option<DecodedArt> {
        self.0.get(url)
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
