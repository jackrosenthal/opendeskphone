//! The command bus: requests for a subsystem to do something.
//!
//! State flows out of subsystems through the store; requests flow into them
//! through here.  Each [`Domain`] has at most one handler (the owning
//! subsystem), which takes [`Envelope`]s from a [`CommandReceiver`] on its
//! own thread or task and answers each one.  Callers use
//! [`crate::Store::send`] (wait for the result, blocking or async) or
//! [`crate::Store::post`] (fire and forget).

use crate::sync::{Pop, Queue};
use crate::types::*;
use crate::Key;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

/// Who handles a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Domain {
    /// Call control (`opendeskphone-sip`).
    Calls = 0,
    /// Audio routing, volume, tones (`opendeskphone-media`).
    Audio,
    /// Key and hook input, notices, screens (`opendeskphone-ui`).
    Ui,
    /// Configuration reload and writes.
    Config,
    /// DPMA provisioning.
    Provisioning,
    Network,
    Apps,
    Update,
    /// Reboot, factory reset, time (the `dphone` main role).
    System,
    /// Multicast paging (the `dphone` main role).
    Paging,
    /// Bluetooth headsets (the `dphone` main role).
    Bluetooth,
    /// The alarm clock (the `dphone` main role).
    Alarms,
}

impl Domain {
    pub const COUNT: usize = 12;
}

/// Which forward rule a [`Command::SetForward`] changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForwardKind {
    Always,
    Busy,
    NoAnswer,
}

/// The content of a sound file.  It prints as its length.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundData(pub Vec<u8>);

impl std::fmt::Debug for SoundData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SoundData({} bytes)", self.0.len())
    }
}

/// A request.  See [`Command::domain`] for who handles each.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    // ---- Calls
    /// Place a call.  `account`: `None` uses the default account.  `path`:
    /// `None` lets the audio policy choose (headset mode, off-hook, else
    /// speaker).  Replies [`Reply::Call`].
    Dial { target: String, account: Option<AccountId>, path: Option<AudioPath> },
    /// Place a call as [`Command::Dial`] does, with extra headers on the
    /// INVITE (an app's `calls.dial`).  Replies [`Reply::Call`].
    DialWith { target: String, account: Option<AccountId>, headers: Vec<(String, String)> },
    Answer { call: CallId, path: Option<AudioPath> },
    /// End a call in any state (rejects a ringing one with 486).
    Hangup { call: CallId },
    /// Decline a ringing call (`busy`: 486, else 603), or silence it.
    Reject { call: CallId, busy: bool },
    /// Stop the ringer for a ringing call without declining it.
    Silence { call: CallId },
    Hold { call: CallId },
    Resume { call: CallId },
    /// Blind transfer (REFER) to a number or URI.
    TransferBlind { call: CallId, target: String },
    /// Attended transfer: connect `call` to the party of `consult`.
    TransferAttended { call: CallId, consult: CallId },
    /// Join two calls into the local three-way conference.
    Conference { a: CallId, b: CallId },
    /// Split the conference back into separate calls (the others held).
    SplitConference { keep: CallId },
    /// Send one DTMF digit (`0-9*#A-D`) on a call, by the negotiated method.
    Dtmf { call: CallId, digit: char },
    /// Make this the focused call (holding the current one if active).
    Focus { call: CallId },
    /// Send the call to the account's voicemail.
    SendToVoicemail { call: CallId },
    /// Park through the PBX (DPMA/feature code, as configured).
    Park { call: CallId },
    /// Pick up the call ringing at a BLF entry's contact: stock's pickup
    /// call, the contact's number with `X-Digium-Call-Feature:
    /// feature_pickup`.
    Pickup { blf_entry: String },
    /// Dial the voicemail number of an account.
    DialVoicemail { account: Option<AccountId> },
    /// Redial the last placed call.
    Redial,
    SetDnd { on: bool },
    /// Empty `target` turns the rule off.
    SetForward { kind: ForwardKind, target: String, no_answer_secs: Option<u16> },
    /// Publish this user's presence.
    SetPresence { status: PresenceStatus, text: String },
    /// Re-register (all accounts if `None`).
    Reregister { account: Option<AccountId> },
    ClearCallLog,
    /// The user looked at the missed calls.
    MarkMissedSeen,
    /// Show rapid-dial page `page`.
    SetRapidDialPage { page: u8 },

    // ---- Audio
    SetAudioPath { path: AudioPath },
    /// Mute or unmute the microphone.
    SetMute { on: bool },
    SetVolume { target: VolumeTarget, level: u8 },
    /// Step a volume up or down.
    AdjustVolume { target: VolumeTarget, delta: i8 },
    /// Latch headset mode on or off.
    SetHeadsetMode { on: bool },
    PlayTone { tone: Tone, duration_ms: u32 },
    StopTone,
    /// Play a ringtone by name once (settings preview).
    PlaySound { name: String },
    /// Play one of the alarm clock's tones by name for a few seconds (a
    /// preview, as [`Command::PlaySound`] is for a ringtone).
    PlayAlarmTone { name: String },
    /// Play a sound once for the app `app`, from `start_secs` in: `sound`
    /// is the content of a sound file, which the app holds in its data.
    /// [`crate::Event::PlaybackStopped`] follows when it ends, is stopped,
    /// or is cut off by the ringer.
    PlayData { sound: SoundData, start_secs: u32, app: String },
    /// Stop a ringtone preview or a sound file.
    StopSound,

    // ---- Ui
    /// A key press or release, as if from the keypad (web screen mirror,
    /// SSH diagnostics, tests).
    InjectKey { key: Key, down: bool },
    /// Lift or replace the handset, as if from the hookswitch.
    InjectHook { off_hook: bool },
    /// A touch of the main screen at pixel (`x`, `y`), as if from the
    /// touchscreen (web screen mirror, SSH diagnostics, tests).  A model
    /// without a touchscreen refuses it.
    InjectTouch { phase: crate::TouchPhase, x: u16, y: u16 },
    /// Raise a notice.  Replies [`Reply::Id`].
    Notify { level: NoticeLevel, title: String, text: String, timeout_ms: u32 },
    DismissNotice { id: u64 },
    SetBacklight { level: u8 },
    SetContrast { level: u8 },
    /// Go to a named native screen.
    ShowScreen { name: String },
    /// Restart the inactivity timer, as a key press does (an app's
    /// `runtime.reset-idle-timer`).
    ResetIdleTimer,

    // ---- Config
    /// Files under nvdata changed: re-read and re-merge.
    ReloadConfig,
    /// Write settings to the user layer (`id`, `value` pairs), then merge.
    SetSettings { settings: Vec<(String, String)> },
    /// Choose a built-in theme by its configuration name (one of Open
    /// Desk Phone's own settings, kept with them, and on the PBX for the
    /// phone's user); an empty name drops the choice made on the phone.
    SetTheme { name: String },
    /// Contact lists changed on disk.
    ReloadContacts,

    // ---- Provisioning
    /// Start (or restart) provisioning; `server` overrides discovery.
    StartProvisioning { server: Option<String> },
    /// Answer `ProvisionPhase::AwaitingUser`.
    SelectUser { id: String, pin: String },
    /// Fetch the configuration again now.
    Reprovision,
    /// Drop the saved server and identity and start over.
    ForgetProvisioning,
    /// Continue without a server (`ProvisionPhase::Standalone`).
    SkipProvisioning,

    // ---- Network
    RenewDhcp,
    /// Re-apply network configuration from the current settings.
    ReconfigureNetwork,
    SyncTime,

    // ---- Apps
    LaunchApp { name: String },
    CloseApp { name: String },
    /// Install a package from a file path.
    InstallApp { path: String },
    RemoveApp { name: String },

    // ---- Update
    /// Install an EFF from a file path.  `allow_stock` must be set to flash
    /// a stock image.
    InstallFirmware { path: String, allow_stock: bool },
    /// Declare the running image good (normally automatic).
    ConfirmTrial,
    /// Switch back to the other bank and reboot.
    Rollback,

    // ---- System
    // ---- Paging
    /// Page a multicast group (`id` of one of `Paging::broadcasts`).  A
    /// call in progress is put on hold first.
    StartPage { id: String },
    /// Stop paging, or stop listening to the page that is playing.
    StopPage,

    // ---- Bluetooth (addresses as text, `00:1B:DC:0F:A1:02`)
    /// Look for devices (`on`), or stop looking.
    BluetoothDiscovery { on: bool },
    /// Pair with a device and connect to it.
    BluetoothPair { addr: String },
    /// Answer the pairing question in `Bluetooth::pairing`.  `pin` is for a
    /// device that needs one typed in.
    BluetoothAnswerPairing { addr: String, accept: bool, pin: Option<String> },
    /// Forget a paired device.
    BluetoothUnpair { addr: String },
    /// Connect to a paired headset.
    BluetoothConnect { addr: String },
    /// Drop the headset connection.
    BluetoothDisconnect,

    // ---- Alarms
    /// Add an alarm (`alarm.id` 0; replies [`Reply::Id`] with its new
    /// identifier) or change the alarm with that identifier.
    SetAlarm { alarm: crate::Alarm },
    RemoveAlarm { id: u32 },
    /// Silence the ringing alarm; it rings again after
    /// [`crate::Alarms::snooze_secs`].
    SnoozeAlarm,
    /// End the ringing alarm, or the snoozed one.
    DismissAlarm,

    Reboot { reason: String },
    FactoryReset,
    /// Recovery only (nvdata could not be mounted): erase the nvdata
    /// partition and reboot, so that the next boot mounts it empty.
    EraseNvdata,
    /// Set the wall clock (Unix seconds).
    SetTime { unix_secs: u64 },
}

impl Command {
    /// The subsystem that handles this command.
    pub fn domain(&self) -> Domain {
        use Command::*;
        match self {
            Dial { .. } | DialWith { .. } | Answer { .. } | Hangup { .. } | Reject { .. } | Silence { .. }
            | Hold { .. } | Resume { .. } | TransferBlind { .. } | TransferAttended { .. }
            | Conference { .. } | SplitConference { .. } | Dtmf { .. } | Focus { .. }
            | SendToVoicemail { .. } | Park { .. } | Pickup { .. } | DialVoicemail { .. }
            | Redial | SetDnd { .. } | SetForward { .. } | SetPresence { .. }
            | Reregister { .. } | ClearCallLog | MarkMissedSeen | SetRapidDialPage { .. } => {
                Domain::Calls
            }
            SetAudioPath { .. } | SetMute { .. } | SetVolume { .. } | AdjustVolume { .. }
            | SetHeadsetMode { .. } | PlayTone { .. } | StopTone | PlaySound { .. } | PlayAlarmTone { .. }
            | PlayData { .. } | StopSound => Domain::Audio,
            InjectKey { .. } | InjectHook { .. } | InjectTouch { .. } | Notify { .. } | DismissNotice { .. }
            | SetBacklight { .. } | SetContrast { .. } | ShowScreen { .. } | ResetIdleTimer => Domain::Ui,
            ReloadConfig | SetSettings { .. } | SetTheme { .. } | ReloadContacts => Domain::Config,
            StartProvisioning { .. } | SelectUser { .. } | Reprovision | ForgetProvisioning
            | SkipProvisioning => Domain::Provisioning,
            RenewDhcp | ReconfigureNetwork | SyncTime => Domain::Network,
            LaunchApp { .. } | CloseApp { .. } | InstallApp { .. } | RemoveApp { .. } => {
                Domain::Apps
            }
            InstallFirmware { .. } | ConfirmTrial | Rollback => Domain::Update,
            Reboot { .. } | FactoryReset | EraseNvdata | SetTime { .. } => Domain::System,
            StartPage { .. } | StopPage => Domain::Paging,
            BluetoothDiscovery { .. } | BluetoothPair { .. } | BluetoothAnswerPairing { .. }
            | BluetoothUnpair { .. } | BluetoothConnect { .. } | BluetoothDisconnect => Domain::Bluetooth,
            SetAlarm { .. } | RemoveAlarm { .. } | SnoozeAlarm | DismissAlarm => Domain::Alarms,
        }
    }
}

/// A successful result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    Done,
    /// The call a `Dial` created.
    Call(CallId),
    /// A new identifier (e.g., a notice).
    Id(u64),
    Text(String),
}

/// Why a command failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandError {
    /// No subsystem has registered for the command's domain.
    NoHandler(Domain),
    /// The handler went away without answering.
    Dropped,
    /// No answer within the caller's timeout.
    Timeout,
    /// The command does not apply now (no such call, wrong state, ...).
    Invalid(String),
    /// The subsystem tried and failed.
    Failed(String),
    /// Refused by policy (e.g., stock firmware without `allow_stock`).
    Denied(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::NoHandler(d) => write!(f, "no handler for {d:?} commands"),
            CommandError::Dropped => f.write_str("handler dropped the command"),
            CommandError::Timeout => f.write_str("timed out waiting for the handler"),
            CommandError::Invalid(s) => write!(f, "invalid: {s}"),
            CommandError::Failed(s) => write!(f, "failed: {s}"),
            CommandError::Denied(s) => write!(f, "denied: {s}"),
        }
    }
}
impl std::error::Error for CommandError {}

/// Result of a command.
pub type CommandResult = Result<Reply, CommandError>;

/// A command delivered to its handler, with the means to answer it.
/// Dropping an envelope unanswered gives the caller
/// [`CommandError::Dropped`].
pub struct Envelope {
    pub command: Command,
    reply: Option<Arc<Queue<CommandResult>>>,
}

impl Envelope {
    /// Answers the command.
    pub fn reply(mut self, result: CommandResult) {
        if let Some(q) = self.reply.take() {
            let _ = q.push(result);
            q.close();
        }
    }
    /// Answers `Ok(Reply::Done)`.
    pub fn done(self) {
        self.reply(Ok(Reply::Done));
    }
    /// Splits into the command and a [`Responder`], for handlers that
    /// answer later.
    pub fn split(mut self) -> (Command, Responder) {
        let reply = self.reply.take();
        let command = std::mem::replace(&mut self.command, Command::Redial);
        (command, Responder { reply })
    }
}

impl Drop for Envelope {
    fn drop(&mut self) {
        if let Some(q) = self.reply.take() {
            q.close();
        }
    }
}

impl fmt::Debug for Envelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Envelope").field("command", &self.command).finish()
    }
}

/// The answering half of an [`Envelope`].
pub struct Responder {
    reply: Option<Arc<Queue<CommandResult>>>,
}

impl Responder {
    pub fn reply(mut self, result: CommandResult) {
        if let Some(q) = self.reply.take() {
            let _ = q.push(result);
            q.close();
        }
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if let Some(q) = self.reply.take() {
            q.close();
        }
    }
}

/// The handler's end for one [`Domain`].  Dropping it unregisters the
/// handler; queued commands then fail with [`CommandError::Dropped`].
pub struct CommandReceiver {
    pub(crate) domain: Domain,
    pub(crate) queue: Arc<Queue<Envelope>>,
}

impl CommandReceiver {
    pub fn domain(&self) -> Domain {
        self.domain
    }
    /// Blocks for the next command.  `None`: replaced by a newer handler.
    pub fn recv(&self) -> Option<Envelope> {
        self.queue.pop(None)
    }
    /// Blocks up to `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Envelope> {
        self.queue.pop(Some(timeout))
    }
    pub fn try_recv(&self) -> Option<Envelope> {
        self.queue.try_pop()
    }
    /// Async form of [`CommandReceiver::recv`]; runtime-independent.
    pub async fn next(&self) -> Option<Envelope> {
        Pop(&self.queue).await
    }
    /// Poll form, for hand-written futures and `select!` loops.
    pub fn poll_next(&self, cx: &mut Context<'_>) -> Poll<Option<Envelope>> {
        self.queue.poll_pop(cx)
    }
}

impl Drop for CommandReceiver {
    fn drop(&mut self) {
        self.queue.close();
        while self.queue.try_pop().is_some() {}
    }
}

/// A command's pending result.  Either block on it or `.await` it.
pub struct Pending {
    state: PendingState,
}

enum PendingState {
    Waiting(Arc<Queue<CommandResult>>),
    Ready(Option<CommandResult>),
}

impl Pending {
    pub(crate) fn ready(r: CommandResult) -> Self {
        Pending { state: PendingState::Ready(Some(r)) }
    }

    /// Blocks until the handler answers.  Do not call from the handler's
    /// own thread, or from an async task (use `.await`).
    pub fn wait(self) -> CommandResult {
        match self.state {
            PendingState::Ready(r) => r.unwrap_or(Err(CommandError::Dropped)),
            PendingState::Waiting(q) => q.pop(None).unwrap_or(Err(CommandError::Dropped)),
        }
    }

    /// Blocks up to `timeout`.
    pub fn wait_timeout(self, timeout: Duration) -> CommandResult {
        match self.state {
            PendingState::Ready(r) => r.unwrap_or(Err(CommandError::Dropped)),
            PendingState::Waiting(q) => match q.pop(Some(timeout)) {
                Some(r) => r,
                None if q.is_closed() => Err(CommandError::Dropped),
                None => Err(CommandError::Timeout),
            },
        }
    }
}

impl Future for Pending {
    type Output = CommandResult;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.state {
            PendingState::Ready(r) => Poll::Ready(r.take().unwrap_or(Err(CommandError::Dropped))),
            PendingState::Waiting(q) => q
                .poll_pop(cx)
                .map(|r| r.unwrap_or(Err(CommandError::Dropped))),
        }
    }
}

pub(crate) struct Bus {
    handlers: std::sync::Mutex<[Option<Arc<Queue<Envelope>>>; Domain::COUNT]>,
}

impl Bus {
    pub(crate) fn new() -> Self {
        Bus { handlers: std::sync::Mutex::new(Default::default()) }
    }

    pub(crate) fn register(&self, domain: Domain) -> CommandReceiver {
        let queue = Queue::new(256);
        let old = self.handlers.lock().unwrap()[domain as usize].replace(queue.clone());
        if let Some(old) = old {
            old.close();
        }
        CommandReceiver { domain, queue }
    }

    pub(crate) fn has_handler(&self, domain: Domain) -> bool {
        self.handlers.lock().unwrap()[domain as usize]
            .as_ref()
            .is_some_and(|q| !q.is_closed())
    }

    pub(crate) fn send(&self, command: Command, want_reply: bool) -> Pending {
        let domain = command.domain();
        let Some(q) = self.handlers.lock().unwrap()[domain as usize].clone() else {
            return Pending::ready(Err(CommandError::NoHandler(domain)));
        };
        let reply = want_reply.then(|| Queue::new(1));
        let env = Envelope { command, reply: reply.clone() };
        match q.push(env) {
            Ok(()) => match reply {
                Some(r) => Pending { state: PendingState::Waiting(r) },
                None => Pending::ready(Ok(Reply::Done)),
            },
            Err(env) => {
                // Handler gone: do not let the envelope's drop close a queue
                // nobody waits on; just report.
                drop(env);
                Pending::ready(Err(CommandError::NoHandler(domain)))
            }
        }
    }
}
