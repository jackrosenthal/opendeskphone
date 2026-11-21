//! Open Desk Phone's state store, event bus, and command bus.
//!
//! This crate is the contract between the subsystems of the `dphone`
//! binary.  It replaces the stock firmware's D-Bus signals; none of those
//! interfaces are reproduced.
//!
//! - **State** ([`State`], one struct per [`Topic`]): what is true now.
//!   Written by the owning subsystem with [`Store::update`], read by
//!   anyone with [`Store::get`] / [`Store::snapshot`], watched with
//!   [`Store::subscribe`].
//! - **Events** ([`Event`]): things that happen (a key press, a call
//!   ending, a server push).  [`Store::emit`] / [`Store::subscribe_events`].
//! - **Commands** ([`Command`]): requests to a subsystem.  The owner calls
//!   [`Store::handle`] for its [`Domain`]; callers use [`Store::send`] or
//!   [`Store::post`].
//! - **Liveness**: [`Store::heartbeat`] and [`Store::set_health`].
//! - **Logs**: [`Store::install_logger`] routes the `log` crate into the
//!   in-memory [`LogRing`].
//!
//! Everything works from plain threads (blocking calls) and from any
//! async runtime (the futures here need no particular executor).
//!
//! ```
//! use opendeskphone_state::*;
//!
//! let store = Store::new(Model::D60);
//!
//! // A subsystem that owns call control:
//! let commands = store.handle(Domain::Calls);
//! let s2 = store.clone();
//! std::thread::spawn(move || {
//!     while let Some(env) = commands.recv() {
//!         match &env.command {
//!             Command::Dial { target, .. } => {
//!                 let id = s2.next_call_id();
//!                 let party = Party { number: target.clone(), ..Default::default() };
//!                 s2.update::<Calls, _>(|c| {
//!                     c.calls.push(Call::new(id, AccountId(0), Direction::Outgoing, party));
//!                     c.focused = Some(id);
//!                 });
//!                 env.reply(Ok(Reply::Call(id)));
//!             }
//!             _ => env.reply(Err(CommandError::Invalid("unsupported".into()))),
//!         }
//!     }
//! });
//!
//! // A watcher (the UI, a streaming RPC, the lamp driver):
//! let sub = store.subscribe(Topic::Calls);
//! let reply = store
//!     .send(Command::Dial { target: "100".into(), account: None, path: None })
//!     .wait();
//! assert!(matches!(reply, Ok(Reply::Call(_))));
//! let wake = sub.wait();
//! assert!(wake.changed(Topic::Calls));
//! assert_eq!(store.get::<Calls>().calls.len(), 1);
//! ```

pub mod clock;
mod command;
mod event;
mod key;
mod logring;
mod model;
mod store;
mod sync;
mod types;
mod version;
mod themes;

pub use command::{
    Command, CommandError, CommandReceiver, CommandResult, Domain, Envelope, ForwardKind,
    Pending, Reply, Responder, SoundData,
};
pub use event::{Event, InputSource};
pub use key::{Key, TouchPhase};
pub use logring::{Level, LogEntry, LogRing};
pub use model::{Family, Model, Size, Skin, Soc, UnknownModel};
pub use store::{CallbackGuard, Changed, Store, Subscription, Wake};
pub use types::*;
pub use version::{BadVersion, FirmwareVersion};
pub use themes::{ThemeName, DEFAULT_THEME, THEMES};

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake as TaskWake, Waker};
    use std::time::Duration;

    struct ThreadWaker(std::thread::Thread);
    impl TaskWake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    /// Minimal executor: enough to prove the futures are runtime-independent.
    fn block_on<F: Future>(f: F) -> F::Output {
        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut f = pin!(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::park();
        }
    }

    #[test]
    fn update_notifies_only_on_change() {
        let store = Store::new(Model::D40);
        let sub = store.subscribe([Topic::Audio, Topic::Calls]);
        assert!(sub.try_take().is_none());
        store.update::<Audio, _>(|a| a.muted = false); // no change
        assert!(sub.try_take().is_none());
        let before = store.get::<Audio>();
        store.update::<Audio, _>(|a| a.muted = true);
        store.update::<Audio, _>(|a| a.speaker_volume = 9);
        let w = sub.try_take().unwrap();
        assert_eq!(w.topics, TopicSet::of(Topic::Audio)); // coalesced
        assert!(!before.muted && store.get::<Audio>().muted); // snapshots immutable
        store.update::<Network, _>(|n| n.phase = NetPhase::Up); // not subscribed
        assert!(sub.try_take().is_none());
        assert!(sub.wait_timeout(Duration::from_millis(10)).is_none());
    }

    #[test]
    fn callbacks() {
        let store = Store::new(Model::D70);
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let guard = store.on_change(Topic::Mwi, move |t| {
            assert_eq!(t, Topic::Mwi);
            n2.fetch_add(1, Ordering::SeqCst);
        });
        let add = |new| {
            store.update::<Mwi, _>(|m| {
                m.accounts = vec![MwiAccount {
                    account: AccountId(0), waiting: true, new, old: 0, urgent_new: 0, urgent_old: 0,
                }]
            })
        };
        add(1);
        add(2);
        assert_eq!(n.load(Ordering::SeqCst), 2);
        drop(guard);
        add(3);
        assert_eq!(n.load(Ordering::SeqCst), 2);
        assert_eq!(store.get::<Mwi>().total_new(), 3);
    }

    #[test]
    fn async_subscription_and_events() {
        let store = Store::new(Model::D60);
        let sub = store.subscribe_events(Topic::Calls);
        let s2 = store.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            s2.emit(Event::Key { key: Key::Soft(1), down: true, source: InputSource::Physical });
        });
        let w = block_on(sub.changed());
        t.join().unwrap();
        assert!(w.topics.is_empty());
        assert_eq!(w.events.len(), 1);
        // Plain subscriptions get no events.
        let plain = store.subscribe(TopicSet::ALL);
        store.emit(Event::TimeChanged);
        assert!(plain.try_take().is_none());
        assert_eq!(sub.try_take().unwrap().events, vec![Event::TimeChanged]);
        // Overflow drops the oldest and counts.
        for _ in 0..300 {
            store.emit(Event::TimeChanged);
        }
        let w = sub.wait();
        assert_eq!(w.events.len(), 256);
        assert_eq!(w.lost_events, 44);
    }

    #[test]
    fn commands_sync_and_async() {
        let store = Store::new(Model::D60);
        assert_eq!(
            store.send(Command::Redial).wait(),
            Err(CommandError::NoHandler(Domain::Calls))
        );
        assert!(!store.post(Command::Redial));
        let rx = store.handle(Domain::Audio);
        assert!(store.has_handler(Domain::Audio));
        let s2 = store.clone();
        let t = std::thread::spawn(move || {
            let mut handled = 0;
            while let Some(env) = block_on(rx.next()) {
                handled += 1;
                match env.command.clone() {
                    Command::SetMute { on } => {
                        s2.update::<Audio, _>(|a| a.muted = on);
                        env.done();
                    }
                    Command::StopTone => drop(env), // unanswered
                    Command::StopSound => {
                        let (_, responder) = env.split();
                        responder.reply(Ok(Reply::Text("later".into())));
                    }
                    Command::SetHeadsetMode { .. } => break,
                    _ => env.reply(Err(CommandError::Invalid("no".into()))),
                }
            }
            handled
        });
        assert_eq!(store.send(Command::SetMute { on: true }).wait(), Ok(Reply::Done));
        assert!(store.get::<Audio>().muted);
        assert_eq!(block_on(store.send(Command::StopTone)), Err(CommandError::Dropped));
        assert_eq!(
            store.send(Command::StopSound).wait_timeout(Duration::from_secs(5)),
            Ok(Reply::Text("later".into()))
        );
        assert!(matches!(
            store.send(Command::SetAudioPath { path: AudioPath::Speaker }).wait(),
            Err(CommandError::Invalid(_))
        ));
        assert!(store.post(Command::SetHeadsetMode { on: true }));
        assert_eq!(t.join().unwrap(), 5);
        // Receiver dropped: handler gone.
        assert!(!store.has_handler(Domain::Audio));
        assert_eq!(
            store.send(Command::StopTone).wait(),
            Err(CommandError::NoHandler(Domain::Audio))
        );
    }

    #[test]
    fn command_timeout() {
        let store = Store::new(Model::D60);
        let _rx = store.handle(Domain::System);
        assert_eq!(
            store.send(Command::FactoryReset).wait_timeout(Duration::from_millis(10)),
            Err(CommandError::Timeout)
        );
    }

    #[test]
    fn health_and_heartbeat() {
        let store = Store::new(Model::D50);
        assert_eq!(store.last_heartbeat(Subsystem::Sip), None);
        store.heartbeat(Subsystem::Sip);
        assert!(store.last_heartbeat(Subsystem::Sip).is_some());
        assert!(store.stale_subsystems(60_000).is_empty());
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(store.stale_subsystems(1), vec![Subsystem::Sip]);
        store.set_health(Subsystem::Net, HealthStatus::Ok);
        store.set_health(Subsystem::Net, HealthStatus::Degraded("no ntp".into()));
        let h = store.get::<Health>();
        assert_eq!(h.subsystems.len(), 1);
        assert_eq!(*h.status(Subsystem::Net), HealthStatus::Degraded("no ntp".into()));
        assert_eq!(*h.status(Subsystem::Web), HealthStatus::Stopped);
    }

    #[test]
    fn log_ring() {
        let store = Store::new(Model::D50);
        let sub = store.subscribe(Topic::Log);
        store.log_line(Level::Info, "t", "one".into());
        store.log_line(Level::Warn, "t", "two".into());
        assert!(sub.try_take().unwrap().changed(Topic::Log));
        let all = store.log().since(0);
        assert_eq!(all.len(), 2);
        assert_eq!(store.log().since(all[0].seq).len(), 1);
        assert_eq!(store.log().tail(1)[0].message, "two");
        assert_eq!(store.log().last_seq(), 2);
        let ring = LogRing::new(3, 1 << 20);
        for i in 0..10 {
            ring.push(Level::Debug, "x", format!("{i}"));
        }
        let t = ring.since(0);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].message, "7");
    }

    #[test]
    fn call_helpers_and_json() {
        let store = Store::new(Model::D65);
        let id = store.next_call_id();
        assert_ne!(id, store.next_call_id());
        store.update::<Calls, _>(|c| {
            let mut call = Call::new(id, AccountId(1), Direction::Incoming, Party::default());
            assert_eq!(call.state, CallState::Ringing);
            call.state = CallState::Held { local: true, remote: false };
            c.calls.push(call);
            c.focused = Some(id);
        });
        let calls = store.get::<Calls>();
        assert!(calls.focused().unwrap().state.is_held());
        assert!(!calls.is_idle());
        let mut log = CallLog::default();
        for _ in 0..150 {
            log.push(CallLogEntry {
                id: 0, kind: CallLogKind::Missed, party: Party::default(),
                account: AccountId(0), start_unix: 0, duration_secs: 0,
            });
        }
        assert_eq!(log.entries.len(), CallLog::CAPACITY);
        assert_eq!(log.entries[0].id, 150);
        assert_eq!(store.model(), Model::D65);
        assert_eq!(TopicSet::ALL.iter().count(), Topic::ALL.len());
    }
}
