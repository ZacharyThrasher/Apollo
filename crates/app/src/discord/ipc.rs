//! Talking to the Discord client over its local IPC pipe, and nothing else: no network, no
//! tokens. The wire format is a little-endian opcode and length followed by a JSON body; the
//! connection starts with a handshake naming the application and then carries `SET_ACTIVITY`
//! commands.
//!
//! A worker thread owns the connection. The UI thread only sends it the activity it wants
//! shown ([`Handle::set`]); the worker coalesces them, keeps within Discord's rate limit, and
//! reconnects quietly when the client is not running or goes away. It never reports errors.

use std::io::{self, Read, Write};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::Activity;

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// Discord accepts five presence updates per twenty seconds.
pub const MIN_INTERVAL: Duration = Duration::from_secs(4);
/// How long to wait before looking for the client again.
pub const RETRY: Duration = Duration::from_secs(30);
/// A frame larger than this is not Discord talking.
const MAX_FRAME: usize = 64 * 1024;

/// One side of the pipe.
pub trait Transport: Read + Write + Send {}
impl<T: Read + Write + Send> Transport for T {}

/// How the worker finds the client. `None` when it is not running.
pub type Connector = Box<dyn FnMut() -> Option<Box<dyn Transport>> + Send>;

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub min_interval: Duration,
    pub retry: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            min_interval: MIN_INTERVAL,
            retry: RETRY,
        }
    }
}

// ---- wire -------------------------------------------------------------------

fn write_frame(io: &mut dyn Transport, op: u32, body: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(body).map_err(io::Error::other)?;
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    io.write_all(&frame)?;
    io.flush()
}

fn read_frame(io: &mut dyn Transport) -> io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    io.read_exact(&mut head)?;
    let op = u32::from_le_bytes(head[..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "oversized frame"));
    }
    let mut body = vec![0u8; len];
    io.read_exact(&mut body)?;
    let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Ok((op, value))
}

/// A connection that has completed the handshake.
struct Client {
    io: Box<dyn Transport>,
    nonce: u64,
}

impl Client {
    fn connect(mut io: Box<dyn Transport>, application_id: &str) -> io::Result<Self> {
        write_frame(
            &mut *io,
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": application_id }),
        )?;
        loop {
            let (op, body) = read_frame(&mut *io)?;
            match op {
                OP_FRAME if body["evt"] == "READY" => return Ok(Self { io, nonce: 0 }),
                // An unknown application id is answered with an ERROR frame, then a close.
                OP_CLOSE | OP_FRAME => {
                    return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "refused"))
                }
                OP_PING => write_frame(&mut *io, OP_PONG, &body)?,
                _ => {}
            }
        }
    }

    /// Show `activity`, or clear the presence for `None`. Waits for Discord's answer. A refusal
    /// of the payload itself (an `ERROR` event) is not a broken connection and is ignored; an
    /// I/O failure is returned.
    fn set_activity(&mut self, activity: Option<&Activity>) -> io::Result<()> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        write_frame(
            &mut *self.io,
            OP_FRAME,
            &json!({
                "cmd": "SET_ACTIVITY",
                "args": {
                    "pid": std::process::id(),
                    "activity": activity.map(Activity::to_json),
                },
                "nonce": nonce,
            }),
        )?;
        loop {
            let (op, body) = read_frame(&mut *self.io)?;
            match op {
                OP_CLOSE => {
                    return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "closed"))
                }
                OP_PING => write_frame(&mut *self.io, OP_PONG, &body)?,
                OP_FRAME if body["nonce"] == nonce.as_str() => return Ok(()),
                _ => {}
            }
        }
    }
}

// ---- worker -------------------------------------------------------------------

enum Command {
    /// What to show; `None` shows nothing (but keeps the connection for the next track).
    Set(Option<Activity>),
    /// Show nothing and let go of the client: the feature is off.
    Release,
    Shutdown,
}

/// The UI side of the worker.
pub struct Handle {
    tx: Sender<Command>,
    done: Receiver<()>,
}

impl Handle {
    pub fn spawn(application_id: String, connector: Connector, timing: Timing) -> Self {
        let (tx, rx) = channel();
        let (done_tx, done) = channel();
        let _: JoinHandle<()> = std::thread::Builder::new()
            .name("discord-presence".into())
            .spawn(move || {
                run(rx, &application_id, connector, timing);
                let _ = done_tx.send(());
            })
            .expect("spawn the presence thread");
        Self { tx, done }
    }

    pub fn set(&self, activity: Option<Activity>) {
        let _ = self.tx.send(Command::Set(activity));
    }

    pub fn release(&self) {
        let _ = self.tx.send(Command::Release);
    }

    /// Clear the presence and stop the worker, waiting up to `wait` for the clear to go out.
    pub fn shutdown(&self, wait: Duration) {
        if self.tx.send(Command::Shutdown).is_ok() {
            let _ = self.done.recv_timeout(wait);
        }
    }
}

fn run(rx: Receiver<Command>, application_id: &str, mut connect: Connector, timing: Timing) {
    let mut wanted: Option<Activity> = None;
    // What Discord shows from this connection.
    let mut shown: Option<Activity> = None;
    let mut client: Option<Client> = None;
    let mut last_send: Option<Instant> = None;
    let mut next_connect = Instant::now();

    loop {
        let now = Instant::now();
        let deadline = match (&client, &wanted) {
            (Some(_), _) if wanted != shown => {
                Some(last_send.map_or(now, |at| at + timing.min_interval))
            }
            (None, Some(_)) => Some(next_connect),
            _ => None,
        };
        let command = match deadline {
            Some(at) if at > now => match rx.recv_timeout(at - now) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            Some(_) => rx.try_recv().ok(),
            None => match rx.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            },
        };
        match command {
            Some(Command::Set(activity)) => {
                wanted = activity;
                continue;
            }
            Some(command @ (Command::Release | Command::Shutdown)) => {
                wanted = None;
                if let Some(mut c) = client.take() {
                    if shown.is_some() {
                        let _ = c.set_activity(None);
                    }
                }
                shown = None;
                next_connect = Instant::now();
                if matches!(command, Command::Shutdown) {
                    return;
                }
                continue;
            }
            None => {}
        }

        let now = Instant::now();
        if client.is_none() && wanted.is_some() && now >= next_connect {
            client = connect()
                .and_then(|io| Client::connect(io, application_id).ok());
            shown = None;
            if client.is_none() {
                next_connect = now + timing.retry;
            }
        }
        if let Some(c) = client.as_mut() {
            let due = last_send.map_or(true, |at| now >= at + timing.min_interval);
            if wanted != shown && due {
                match c.set_activity(wanted.as_ref()) {
                    Ok(()) => {
                        shown = wanted.clone();
                        last_send = Some(Instant::now());
                    }
                    Err(_) => {
                        client = None;
                        shown = None;
                        next_connect = Instant::now() + timing.retry;
                    }
                }
            }
        }
    }
    // The UI side went away: the pipe closing makes Discord drop the presence anyway.
}
