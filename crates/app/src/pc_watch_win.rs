//! The Windows end of `pc_watch.rs`: a background thread that looks at Core Audio every
//! [`POLL`] (the default playback device, and which other processes are playing sound), and a
//! suspend/resume notification. What it sees goes through [`PcWatch`], which decides; what that
//! decides arrives on the UI thread as a [`PcEvent`] for the player.
//!
//! Polling instead of Core Audio's notification interfaces keeps this free of hand-written COM
//! objects: a quarter of a second is quick enough to pause before anyone notices, and an
//! enumeration of a handful of sessions costs nothing.

use std::ffi::c_void;
use std::time::{Instant, SystemTime};

use gpui::{App, Entity};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use windows::core::{Interface, PWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    eConsole, eRender, AudioSessionStateActive, IAudioSessionControl, IAudioSessionControl2,
    IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Power::{
    PowerRegisterSuspendResumeNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
};
use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;

use crate::pc_watch::{PcEvent, PcWatch, Snapshot, Sound, POLL};
use crate::player::PlayerModel;
use crate::settings::{SettingsModel, PAUSE_ON_OTHER_AUDIO};

/// A session this quiet (peak level at or below it) is not playing anything.
const SILENCE: f32 = 0.001;

const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_APMRESUMESUSPEND: u32 = 0x0007;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;

/// Starts watching: the PC's events reach `player` for as long as the app runs. "Pause when other
/// audio plays" is read from `settings` when the event arrives, so flipping it takes effect at
/// once.
pub fn install(cx: &mut App, player: Entity<PlayerModel>, settings: Entity<SettingsModel>) {
    let (tx, mut rx) = unbounded_channel::<PcEvent>();
    register_suspend_resume(tx.clone());
    let spawned = std::thread::Builder::new()
        .name("apollo-pc-watch".into())
        .spawn(move || watch(tx));
    if let Err(err) = spawned {
        eprintln!("pc watch: could not start: {err}");
    }
    let weak = player.downgrade();
    cx.spawn(async move |cx| {
        while let Some(event) = rx.recv().await {
            cx.update(|cx| match event {
                PcEvent::OutputLost => {
                    let _ = weak.update(cx, |m, cx| m.output_lost(cx));
                }
                PcEvent::OtherAudio => {
                    if settings.read(cx).get(&PAUSE_ON_OTHER_AUDIO) {
                        let _ = weak.update(cx, |m, cx| m.other_audio(cx));
                    }
                }
            });
        }
    })
    .detach();
}

// ------------------------------------------------------------------ sleep and wake

/// Asks Windows to call [`on_power`] when the PC suspends or resumes. The notice comes on a
/// system thread, so it only passes a message on. Registration lasts as long as the process, so
/// the context and the parameters are leaked on purpose.
fn register_suspend_resume(tx: UnboundedSender<PcEvent>) {
    let context = Box::into_raw(Box::new(tx)) as *const c_void;
    let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
        Callback: Some(on_power),
        Context: context as *mut c_void,
    }));
    let mut registration: *mut c_void = std::ptr::null_mut();
    // SAFETY: `params` and `context` live for the whole process; the callback only reads them.
    let status = unsafe {
        PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS as *mut c_void),
            &mut registration,
        )
    };
    if status.0 != 0 {
        eprintln!("pc watch: could not register for suspend and resume ({status:?})");
    }
}

unsafe extern "system" fn on_power(context: *const c_void, kind: u32, _setting: *const c_void) -> u32 {
    if matches!(
        kind,
        PBT_APMSUSPEND | PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND
    ) && !context.is_null()
    {
        // SAFETY: the context is the leaked sender registered above and is never freed.
        let tx = unsafe { &*(context as *const UnboundedSender<PcEvent>) };
        let _ = tx.send(PcEvent::OutputLost);
    }
    0
}

// ------------------------------------------------------------------ the polling thread

/// What one look at the default playback device found.
struct Audio {
    enumerator: IMMDeviceEnumerator,
    /// The session manager of the device `id` names, kept until the default device changes.
    manager: Option<(String, IAudioSessionManager2)>,
    own_process: u32,
}

impl Audio {
    fn new() -> windows::core::Result<Self> {
        // SAFETY: plain COM activation on a thread that initialised COM.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        Ok(Self {
            enumerator,
            manager: None,
            own_process: std::process::id(),
        })
    }

    /// The default playback device's id and the other processes' sound on it. `None` for the
    /// device when there is none.
    fn look(&mut self) -> (Option<String>, Vec<Sound>) {
        // SAFETY: COM calls on interfaces this thread created.
        unsafe {
            let Ok(device) = self.enumerator.GetDefaultAudioEndpoint(eRender, eConsole) else {
                self.manager = None;
                return (None, Vec::new());
            };
            let Ok(id) = device.GetId().map(|id| take_string(id)) else {
                self.manager = None;
                return (None, Vec::new());
            };
            if self.manager.as_ref().map(|(known, _)| known) != Some(&id) {
                self.manager = device
                    .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
                    .ok()
                    .map(|m| (id.clone(), m));
            }
            let sounds = match &self.manager {
                Some((_, manager)) => sounds_of(manager, self.own_process),
                None => Vec::new(),
            };
            (Some(id), sounds)
        }
    }
}

/// Copies a COM-allocated string and frees it.
///
/// # Safety
/// `text` must be a string Core Audio allocated for the caller (it is freed here).
unsafe fn take_string(text: PWSTR) -> String {
    // SAFETY: per the contract above.
    unsafe {
        let s = text.to_string().unwrap_or_default();
        CoTaskMemFree(Some(text.0 as *const c_void));
        s
    }
}

/// Which other processes are playing sound on `manager`'s device right now: sessions that are
/// active with a level above silence, leaving out Apollo itself and the system-sounds session.
///
/// # Safety
/// COM calls; call on the thread that created `manager`.
unsafe fn sounds_of(manager: &IAudioSessionManager2, own_process: u32) -> Vec<Sound> {
    let mut by_process: Vec<Sound> = Vec::new();
    // SAFETY: per the contract above.
    unsafe {
        let Ok(sessions) = manager.GetSessionEnumerator() else {
            return by_process;
        };
        let count = sessions.GetCount().unwrap_or(0);
        for i in 0..count {
            let Ok(control) = sessions.GetSession(i) else {
                continue;
            };
            let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                continue;
            };
            // S_OK means "this is the system-sounds session" (S_FALSE, also a success code, means
            // it is not).
            if control2.IsSystemSoundsSession().0 == 0 {
                continue;
            }
            let Ok(process) = control2.GetProcessId() else {
                continue;
            };
            if process == 0 || process == own_process {
                continue;
            }
            let audible = is_audible(&control);
            match by_process.iter_mut().find(|s| s.process == process) {
                Some(s) => s.audible |= audible,
                None => by_process.push(Sound { process, audible }),
            }
        }
    }
    by_process
}

/// Active, and the level is above silence (a paused or muted stream keeps a session open).
///
/// # Safety
/// COM calls; call on the thread that created `control`.
unsafe fn is_audible(control: &IAudioSessionControl) -> bool {
    // SAFETY: per the contract above.
    unsafe {
        if control.GetState().ok() != Some(AudioSessionStateActive) {
            return false;
        }
        control
            .cast::<IAudioMeterInformation>()
            .and_then(|meter| meter.GetPeakValue())
            .is_ok_and(|peak| peak > SILENCE)
    }
}

fn watch(tx: UnboundedSender<PcEvent>) {
    // SAFETY: initialises COM for this thread, which owns every interface below.
    let initialised = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if initialised.is_err() {
        eprintln!("pc watch: could not initialise COM: {initialised:?}");
        return;
    }
    let mut audio = match Audio::new() {
        Ok(audio) => audio,
        Err(err) => {
            eprintln!("pc watch: no audio device enumerator: {err}");
            return;
        }
    };
    let mut watch = PcWatch::new();
    loop {
        let (device, sounds) = audio.look();
        let events = watch.poll(
            Instant::now(),
            &Snapshot {
                device: device.as_deref(),
                sounds: &sounds,
                wall: SystemTime::now(),
            },
        );
        for event in events {
            if tx.send(event).is_err() {
                // The app is gone.
                return;
            }
        }
        std::thread::sleep(POLL);
    }
}
