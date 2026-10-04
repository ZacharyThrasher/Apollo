use std::time::{Duration, Instant};

use super::*;

fn track(id: &str) -> MediaTrack {
    MediaTrack {
        video_id: id.to_owned(),
        title: format!("Title {id}"),
        artist: "Some Artist".to_owned(),
        album: "Some Album".to_owned(),
        duration_ms: Some(200_000),
    }
}

fn state(id: &str, playing: bool, position_ms: u64) -> MediaState {
    MediaState {
        track: Some(track(id)),
        playing,
        position_ms,
        cover: None,
    }
}

const ART: &str = "https://lh3.googleusercontent.com/abc=w720-h720";

#[derive(Debug, PartialEq)]
enum Call {
    Set(Option<Activity>),
    Release,
}

#[derive(Default)]
struct Recorder(Vec<Call>);

impl PresenceSink for Recorder {
    fn set(&mut self, activity: Option<Activity>) {
        self.0.push(Call::Set(activity));
    }

    fn release(&mut self) {
        self.0.push(Call::Release);
    }
}

struct Rig {
    bridge: PresenceBridge<Recorder>,
    now: Instant,
    unix_ms: u64,
}

impl Rig {
    fn new() -> Self {
        Self {
            bridge: PresenceBridge::new(Recorder::default()),
            now: Instant::now(),
            unix_ms: 1_700_000_000_000,
        }
    }

    fn apply(&mut self, enabled: bool, state: &MediaState) {
        self.bridge
            .apply(enabled, state, Some(ART), self.now, self.unix_ms);
    }

    fn pass(&mut self, ms: u64) {
        self.now += Duration::from_millis(ms);
        self.unix_ms += ms;
    }

    fn calls(&self) -> usize {
        self.bridge.sink().0.len()
    }
}

#[test]
fn the_activity_carries_title_artist_album_art_progress_and_one_button() {
    let activity = Activity::of(&track("abc123"), Some(ART), 30_000, 1_700_000_000_000);
    assert_eq!(
        activity.to_json(),
        json!({
            "type": 2,
            "details": "Title abc123",
            "state": "Some Artist",
            "timestamps": { "start": 1_699_999_970u64, "end": 1_700_000_170u64 },
            "assets": { "large_image": ART, "large_text": "Some Album" },
            "buttons": [{
                "label": "Play on YouTube Music",
                "url": "https://music.youtube.com/watch?v=abc123",
            }],
        })
    );
}

#[test]
fn missing_pieces_are_left_out_rather_than_sent_blank() {
    let mut t = track("abc123");
    t.artist = "  ".into();
    t.album = String::new();
    t.duration_ms = None;
    let json = Activity::of(&t, None, 0, 1_700_000_000_000).to_json();
    assert!(json.get("state").is_none());
    assert!(json.get("assets").is_none());
    assert!(json["timestamps"].get("end").is_none());
    // A pictureless presence still shows the button.
    assert!(json["buttons"].is_array());
    // A video id that is not URL-safe gets no button; an image that is not https is dropped.
    t.video_id = "a b?c".into();
    let json = Activity::of(&t, Some("http://example.com/a.png"), 0, 0).to_json();
    assert!(json.get("buttons").is_none());
    assert!(json.get("assets").is_none());
}

#[test]
fn text_fits_what_discord_accepts() {
    assert_eq!(text_field("   "), None);
    assert_eq!(text_field("OK").as_deref(), Some("OK"));
    // One character is padded to two, invisibly.
    assert_eq!(text_field("I").unwrap().chars().count(), 2);
    let long = text_field(&"x".repeat(500)).unwrap();
    assert!(long.chars().count() <= 128);
    assert!(long.ends_with('…'));
    // An image URL past Discord's limit is left out.
    let huge = format!("https://example.com/{}", "a".repeat(300));
    assert!(Activity::of(&track("a"), Some(&huge), 0, 0)
        .large_image
        .is_none());
}

#[test]
fn playing_shows_once_and_ticks_do_not_resend() {
    let mut rig = Rig::new();
    rig.apply(true, &state("a", true, 5_000));
    // Position advancing in step with the clock is not a change.
    for i in 1..=30u64 {
        rig.pass(1000);
        rig.apply(true, &state("a", true, 5_000 + i * 1000));
    }
    assert_eq!(rig.calls(), 1);
}

#[test]
fn pause_and_stop_clear_and_resume_shows_again() {
    let mut rig = Rig::new();
    rig.apply(true, &state("a", true, 0));
    rig.pass(10_000);
    rig.apply(true, &state("a", false, 10_000));
    assert_eq!(rig.bridge.sink().0.last(), Some(&Call::Set(None)));
    // Staying paused (or stopping afterwards) sends nothing more.
    rig.pass(5_000);
    rig.apply(true, &state("a", false, 10_000));
    rig.apply(true, &MediaState::default());
    assert_eq!(rig.calls(), 2);
    // Resuming restates the progress bar from the paused position.
    rig.pass(5_000);
    rig.apply(true, &state("a", true, 10_000));
    assert_eq!(rig.calls(), 3);
    match rig.bridge.sink().0.last() {
        Some(Call::Set(Some(a))) => assert_eq!(a.start, (rig.unix_ms - 10_000) / 1000),
        other => panic!("expected an activity, got {other:?}"),
    }
}

#[test]
fn a_new_track_or_a_seek_updates_the_presence() {
    let mut rig = Rig::new();
    rig.apply(true, &state("a", true, 0));
    rig.pass(5_000);
    rig.apply(true, &state("b", true, 0));
    assert_eq!(rig.calls(), 2);
    rig.pass(1_000);
    // A jump forward of a minute.
    rig.apply(true, &state("b", true, 61_000));
    assert_eq!(rig.calls(), 3);
    // The art arriving late (or changing) restates it too.
    rig.pass(1_000);
    rig.bridge
        .apply(true, &state("b", true, 62_000), None, rig.now, rig.unix_ms);
    assert_eq!(rig.calls(), 4);
}

#[test]
fn a_stall_that_lets_the_bar_run_ahead_is_corrected() {
    let mut rig = Rig::new();
    rig.apply(true, &state("a", true, 0));
    // Ten seconds pass but the audio only advanced two.
    rig.pass(10_000);
    rig.apply(true, &state("a", true, 2_000));
    assert_eq!(rig.calls(), 2);
}

#[test]
fn off_clears_once_and_stays_quiet() {
    let mut rig = Rig::new();
    // Off from the start: no calls, so no connection is ever made.
    rig.apply(false, &state("a", true, 0));
    assert_eq!(rig.calls(), 0);
    rig.apply(true, &state("a", true, 0));
    rig.apply(false, &state("a", true, 1_000));
    rig.apply(false, &state("a", true, 2_000));
    assert_eq!(rig.bridge.sink().0.last(), Some(&Call::Release));
    assert_eq!(rig.calls(), 2);
    // Back on while still playing shows it again.
    rig.pass(2_000);
    rig.apply(true, &state("a", true, 4_000));
    assert_eq!(rig.calls(), 3);
}

#[test]
fn nothing_playing_when_on_sends_nothing() {
    let mut rig = Rig::new();
    rig.apply(true, &MediaState::default());
    rig.apply(true, &state("a", false, 0));
    assert_eq!(rig.calls(), 0);
}

#[cfg(unix)]
mod wire {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use super::*;
    use crate::discord::ipc::{Connector, Handle, Timing, Transport};

    #[derive(Debug)]
    enum Seen {
        Handshake(Value),
        Activity(Value),
        Closed,
    }

    fn frame(op: u32, body: &Value) -> Vec<u8> {
        let body = serde_json::to_vec(body).unwrap();
        let mut out = op.to_le_bytes().to_vec();
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    fn read(stream: &mut UnixStream) -> Option<(u32, Value)> {
        let mut head = [0u8; 8];
        stream.read_exact(&mut head).ok()?;
        let op = u32::from_le_bytes(head[..4].try_into().unwrap());
        let len = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).ok()?;
        Some((op, serde_json::from_slice(&body).unwrap()))
    }

    /// A stand-in Discord on one end of a socket pair. `ping_first` makes it send a ping before
    /// answering a command, as the real client does now and then.
    fn fake_discord(
        mut stream: UnixStream,
        ping_first: bool,
        hang_up_after: Option<usize>,
        seen: std::sync::mpsc::Sender<Seen>,
    ) {
        thread::spawn(move || {
            let Some((0, hello)) = read(&mut stream) else {
                return;
            };
            let _ = seen.send(Seen::Handshake(hello));
            let ready = json!({ "cmd": "DISPATCH", "evt": "READY", "data": { "v": 1 } });
            stream.write_all(&frame(1, &ready)).unwrap();
            let mut answered = 0usize;
            while let Some((op, body)) = read(&mut stream) {
                assert_eq!(op, 1);
                assert_eq!(body["cmd"], "SET_ACTIVITY");
                assert_eq!(body["args"]["pid"], std::process::id());
                if ping_first {
                    stream.write_all(&frame(3, &json!({ "n": 1 }))).unwrap();
                    let (op, _) = read(&mut stream).expect("a pong");
                    assert_eq!(op, 4);
                }
                let _ = seen.send(Seen::Activity(body["args"]["activity"].clone()));
                let reply = json!({ "cmd": "SET_ACTIVITY", "evt": null, "nonce": body["nonce"] });
                stream.write_all(&frame(1, &reply)).unwrap();
                answered += 1;
                if hang_up_after == Some(answered) {
                    break;
                }
            }
            let _ = seen.send(Seen::Closed);
        });
    }

    /// A connector that fails `refusals` times, then hands out a fake Discord.
    fn connector(refusals: usize, ping_first: bool) -> (Connector, Receiver<Seen>) {
        let (tx, rx) = channel();
        let left = Arc::new(Mutex::new(refusals));
        let connector: Connector = Box::new(move || {
            let mut left = left.lock().unwrap();
            if *left > 0 {
                *left -= 1;
                return None;
            }
            let (ours, theirs) = UnixStream::pair().unwrap();
            fake_discord(theirs, ping_first, None, tx.clone());
            Some(Box::new(ours) as Box<dyn Transport>)
        });
        (connector, rx)
    }

    fn quick() -> Timing {
        Timing {
            min_interval: Duration::from_millis(150),
            retry: Duration::from_millis(100),
        }
    }

    fn activity(n: u64) -> Activity {
        Activity::of(&track(&format!("v{n}")), Some(ART), 0, n * 1000)
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn handshake_names_the_app_then_the_activity_follows() {
        let (connector, seen) = connector(0, false);
        let handle = Handle::spawn("12345".into(), connector, quick());
        handle.set(Some(activity(1)));
        match seen.recv_timeout(WAIT).unwrap() {
            Seen::Handshake(hello) => assert_eq!(hello, json!({ "v": 1, "client_id": "12345" })),
            other => panic!("expected the handshake, got {other:?}"),
        }
        match seen.recv_timeout(WAIT).unwrap() {
            Seen::Activity(a) => assert_eq!(a, activity(1).to_json()),
            other => panic!("expected the activity, got {other:?}"),
        }
        // Pause: the presence is cleared with a null activity.
        handle.set(None);
        match seen.recv_timeout(WAIT).unwrap() {
            Seen::Activity(a) => assert!(a.is_null()),
            other => panic!("expected the clear, got {other:?}"),
        }
        handle.shutdown(WAIT);
    }

    #[test]
    fn a_ping_in_the_middle_is_answered() {
        let (connector, seen) = connector(0, true);
        let handle = Handle::spawn("1".into(), connector, quick());
        handle.set(Some(activity(1)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Handshake(_)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Activity(_)));
        handle.shutdown(WAIT);
    }

    #[test]
    fn a_burst_of_updates_is_spaced_and_only_the_latest_is_sent() {
        let (connector, seen) = connector(0, false);
        let handle = Handle::spawn("1".into(), connector, quick());
        handle.set(Some(activity(1)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Handshake(_)));
        let first = Instant::now();
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Activity(_)));
        handle.set(Some(activity(2)));
        handle.set(Some(activity(3)));
        handle.set(Some(activity(4)));
        match seen.recv_timeout(WAIT).unwrap() {
            Seen::Activity(a) => assert_eq!(a, activity(4).to_json()),
            other => panic!("expected the latest activity, got {other:?}"),
        }
        assert!(first.elapsed() >= quick().min_interval);
        assert!(seen.recv_timeout(Duration::from_millis(400)).is_err());
        handle.shutdown(WAIT);
    }

    #[test]
    fn a_missing_client_is_retried_quietly_until_it_appears() {
        let (connector, seen) = connector(3, false);
        let handle = Handle::spawn("1".into(), connector, quick());
        handle.set(Some(activity(1)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Handshake(_)));
        match seen.recv_timeout(WAIT).unwrap() {
            Seen::Activity(a) => assert_eq!(a, activity(1).to_json()),
            other => panic!("expected the activity, got {other:?}"),
        }
        handle.shutdown(WAIT);
    }

    #[test]
    fn nothing_is_connected_while_nothing_is_wanted() {
        let (connector, seen) = connector(0, false);
        let handle = Handle::spawn("1".into(), connector, quick());
        handle.set(None);
        assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
        handle.shutdown(WAIT);
    }

    #[test]
    fn release_clears_and_hangs_up_and_shutdown_clears_too() {
        let (connector, seen) = connector(0, false);
        let handle = Handle::spawn("1".into(), connector, quick());
        handle.set(Some(activity(1)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Handshake(_)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Activity(_)));
        handle.release();
        match seen.recv_timeout(WAIT).unwrap() {
            Seen::Activity(a) => assert!(a.is_null()),
            other => panic!("expected the clear, got {other:?}"),
        }
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Closed));

        // Turned back on: a fresh connection, then quitting clears what it showed.
        handle.set(Some(activity(2)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Handshake(_)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Activity(_)));
        handle.shutdown(WAIT);
        match seen.recv_timeout(WAIT).unwrap() {
            Seen::Activity(a) => assert!(a.is_null()),
            other => panic!("expected the clear on quit, got {other:?}"),
        }
    }

    #[test]
    fn a_client_that_goes_away_is_found_again() {
        let (tx, seen) = channel();
        let connector: Connector = Box::new(move || {
            let (ours, theirs) = UnixStream::pair().unwrap();
            // Every Discord this finds hangs up after answering once.
            fake_discord(theirs, false, Some(1), tx.clone());
            Some(Box::new(ours) as Box<dyn Transport>)
        });
        let handle = Handle::spawn("1".into(), connector, quick());
        handle.set(Some(activity(1)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Handshake(_)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Activity(_)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Closed));
        // The next track finds the client gone, reconnects after the quiet retry, and shows.
        handle.set(Some(activity(2)));
        assert!(matches!(seen.recv_timeout(WAIT).unwrap(), Seen::Handshake(_)));
        match seen.recv_timeout(WAIT).unwrap() {
            Seen::Activity(a) => assert_eq!(a, activity(2).to_json()),
            other => panic!("expected the activity, got {other:?}"),
        }
        handle.shutdown(WAIT);
    }
}
