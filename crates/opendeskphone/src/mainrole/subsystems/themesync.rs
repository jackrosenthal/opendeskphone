//! The user's theme, kept on the PBX: it follows the user to a phone that
//! was reset, and to another phone of theirs.
//!
//! The theme is a setting of the phone (`theme` in Open Desk Phone's settings
//! file).  The PBX keeps a copy for the phone's user in its store of app data
//! (Open DPMA), which the apps use through `storage` and the firmware reaches
//! here through the same [`PbxStore`]: the user's key `theme` of the name
//! `opendeskphone` ([`opendeskphone_apps::manifest::PHONE_DATA_NAME`], which
//! no app can have), whose value is the theme's name.
//!
//! The settings file also says what the phone and the PBX last agreed on
//! (`theme_sync`: the theme, the version of the PBX's entry, and the
//! user).  A pass ([`sync`]) reads the PBX's entry and compares both sides
//! with that:
//!
//! - Neither changed: nothing to do.
//! - Only the phone's theme changed (it was chosen here): it is sent, to
//!   replace exactly the entry that was read.  An empty choice removes the
//!   entry.
//! - Only the PBX's entry changed (the user chose on another phone, or
//!   this phone was reset and has no record): the phone takes it.
//! - Both changed: the later choice stays, by the entry's time on the PBX
//!   and the time the phone first saw its theme differ.  When the phone
//!   did not know the time then (its clock was not set), its own choice
//!   stays.
//!
//! With another user on the phone than the record is about, a theme that
//! user has on the PBX is taken; if they have none, the phone's theme
//! stays and is not sent as theirs.
//!
//! A pass runs when the store comes into reach (once for each DPMA
//! session, after provisioning) and when the configuration changes.  A
//! PBX without the store (the official DPMA), or one out of reach, leaves
//! everything as it is: the theme is the phone's setting, chosen and kept
//! there as any other, and what was chosen meanwhile is sent when the
//! store is next in reach.  The store does not tell a phone of a change
//! to a user's keys, so a choice made on another phone arrives with the
//! next session.
//!
//! Only the color models have themes; on the others nothing runs.

use std::sync::Arc;

use opendeskphone_apps::manifest::PHONE_DATA_NAME;
use opendeskphone_apps::services::{Completion, PbxStore, StoreEntry, StoreOp, StoreReply, StoreScope};
use opendeskphone_config::{Config, ThemeSync};
use opendeskphone_state::System;
use log::{debug, info, warn};

use super::SharedConfig;
use super::system::{ConfigHub, write_config};
use crate::mainrole::Context;

/// The key of the theme among the user's keys.
pub const KEY: &str = "theme";

/// How a pass ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The phone and the PBX have the same theme, as before.
    InSync,
    /// The phone's theme was sent to the PBX (`None`: its entry was
    /// removed).
    Sent(Option<String>),
    /// The PBX's theme was taken (`None`: the choice was dropped).
    Taken(Option<String>),
    /// The PBX's store was not reached, or it refused.
    Unreachable(String),
    /// The phone has no themes.
    NoThemes,
}

/// A theme's name as the PBX may hold it: what the configuration can
/// name (letters, digits, and hyphens; the phone shows the default theme
/// for a name it does not have).
fn valid(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

async fn ask(store: &dyn PbxStore, op: StoreOp) -> StoreReply {
    let (tx, rx) = tokio::sync::oneshot::channel();
    store.request(
        op,
        Completion::new(move |reply| {
            let _ = tx.send(reply);
        }),
    );
    rx.await.unwrap_or_else(|_| StoreReply::Unreachable("the request was dropped".into()))
}

/// The phone's side: the theme chosen here, and the record.
fn local(config: &SharedConfig) -> (bool, Option<String>, ThemeSync) {
    let cfg = config.lock().unwrap().config();
    let settings = cfg.opendeskphone_settings();
    (cfg.model().has_themes(), settings.local_theme(), settings.theme_sync.as_option().cloned().unwrap_or_default())
}

/// Writes the record, and with `take` the theme too, unless the theme was
/// changed on the phone since `seen` was read (the next pass sends it).
async fn save(
    config: &SharedConfig,
    seen: Option<String>,
    take: Option<Option<String>>,
    record: ThemeSync,
) -> Result<Option<Arc<Config>>, String> {
    write_config(config, move |store| {
        if store.config().opendeskphone_settings().local_theme() != seen {
            return Ok(None);
        }
        store
            .update_opendeskphone_settings(|s| {
                if let Some(theme) = take {
                    s.theme = theme;
                }
                s.theme_sync = Some(record).into();
            })
            .map(Some)
    })
    .await
    .map_err(|e| format!("{e:?}"))
}

/// One pass (see the module's description).  `now_unix` is the time, if
/// the phone knows it.  Returns how it ended and, when the settings file
/// was written, the configuration since.
pub async fn sync(config: &SharedConfig, store: &dyn PbxStore, now_unix: Option<u64>) -> (Outcome, Option<Arc<Config>>) {
    let (has_themes, mut theme, mut record) = local(config);
    if !has_themes {
        return (Outcome::NoThemes, None);
    }
    let mut written = None;
    // When the theme was first seen to differ from the one agreed on.
    let differs = theme != record.theme;
    let stamp = match (differs, record.changed_unix) {
        (true, 0) => now_unix.unwrap_or(0),
        (true, at) => at,
        (false, _) => 0,
    };
    if stamp != record.changed_unix {
        record.changed_unix = stamp;
        match save(config, theme.clone(), None, record.clone()).await {
            Ok(Some(cfg)) => written = Some(cfg),
            Ok(None) => {}
            Err(e) => warn!("theme: the settings were not written: {e}"),
        }
    }

    let get = StoreOp::Get { app: PHONE_DATA_NAME.into(), scope: StoreScope::User, key: KEY.into() };
    let mut entry: Option<StoreEntry> = match ask(store, get).await {
        StoreReply::Entry(entry) => entry,
        StoreReply::Unreachable(why) | StoreReply::Refused(why) => return (Outcome::Unreachable(why), written),
        other => return (Outcome::Unreachable(format!("the PBX answered {other:?}")), written),
    };
    let user = store.user().map(|u| u.id).unwrap_or_default();
    // A write that finds another entry than the one read is shown that
    // entry, and the pass goes on with it.
    for _ in 0..4 {
        // The phone's side may have changed while the PBX was asked.
        (_, theme, _) = local(config);
        let same_user = !record.user.is_empty() && record.user == user;
        let version = entry.as_ref().map_or(0, |e| e.version);
        // The entry's theme.  An entry that is not a theme's name counts
        // as the theme the phone has: it is not taken, and it is replaced
        // when the phone has a choice to send.
        let theirs = match &entry {
            Some(e) => match std::str::from_utf8(&e.value) {
                Ok(name) if valid(name) => Some(name.to_string()),
                _ => theme.clone(),
            },
            None => None,
        };
        let ours_changed = theme != record.theme;
        let theirs_changed = if same_user { version != record.version } else { entry.is_some() };
        let send = match (ours_changed, theirs_changed) {
            (false, false) => {
                if !same_user || record.changed_unix != 0 {
                    // The first pass for this user: from here on, a
                    // choice on either side is a change.
                    let agreed = ThemeSync { user, theme: theme.clone(), version, changed_unix: 0 };
                    if let Ok(Some(cfg)) = save(config, theme, None, agreed).await {
                        written = Some(cfg);
                    }
                }
                return (Outcome::InSync, written);
            }
            (true, false) => true,
            (false, true) => false,
            // The later choice; the phone's, when it does not know when
            // it was made, or when the PBX's entry was removed.
            (true, true) => match &entry {
                Some(e) => record.changed_unix == 0 || e.modified < record.changed_unix,
                None => true,
            },
        };
        if !send {
            let agreed = ThemeSync { user, theme: theirs.clone(), version, changed_unix: 0 };
            if theirs == theme {
                // The same on both sides already: only the record.
                if let Ok(Some(cfg)) = save(config, theme, None, agreed).await {
                    written = Some(cfg);
                }
                return (Outcome::InSync, written);
            }
            return match save(config, theme, Some(theirs.clone()), agreed).await {
                Ok(Some(cfg)) => (Outcome::Taken(theirs), Some(cfg)),
                // Chosen anew on the phone meanwhile: the next pass.
                Ok(None) => (Outcome::InSync, written),
                Err(e) => (Outcome::Unreachable(format!("the settings were not written: {e}")), written),
            };
        }
        let op = match &theme {
            Some(name) => StoreOp::Set {
                app: PHONE_DATA_NAME.into(),
                scope: StoreScope::User,
                key: KEY.into(),
                value: name.clone().into_bytes(),
                expect: Some(version),
            },
            None => StoreOp::Remove { app: PHONE_DATA_NAME.into(), scope: StoreScope::User, key: KEY.into(), expect: Some(version) },
        };
        let version = match ask(store, op).await {
            StoreReply::Stored(e) => e.version,
            StoreReply::Removed => 0,
            StoreReply::Conflict(current) => {
                entry = current;
                continue;
            }
            StoreReply::Unreachable(why) | StoreReply::Refused(why) => return (Outcome::Unreachable(why), written),
            other => return (Outcome::Unreachable(format!("the PBX answered {other:?}")), written),
        };
        let agreed = ThemeSync { user, theme: theme.clone(), version, changed_unix: 0 };
        if let Ok(Some(cfg)) = save(config, theme.clone(), None, agreed).await {
            written = Some(cfg);
        }
        return (Outcome::Sent(theme), written);
    }
    (Outcome::Unreachable("the PBX's entry kept changing".into()), written)
}

/// Runs a pass whenever the store comes into reach or the configuration
/// changes, for as long as the phone runs.
pub fn start(ctx: &Context, config: Option<SharedConfig>, hub: &ConfigHub, store: Arc<dyn PbxStore>, online: Arc<tokio::sync::Notify>) {
    let Some(config) = config else { return };
    if !config.lock().unwrap().config().model().has_themes() {
        return;
    }
    // One notification for both reasons.
    let kick = online;
    {
        let kick = kick.clone();
        hub.on_change(move |_| kick.notify_one());
    }
    let (state, hub) = (ctx.store.clone(), hub.clone());
    tokio::spawn(async move {
        loop {
            kick.notified().await;
            let now = state.read::<System, _>(|s| s.time_synced).then(opendeskphone_state::clock::unix_secs);
            let (outcome, written) = sync(&config, &*store, now).await;
            match &outcome {
                Outcome::Sent(theme) => info!("theme: {} is on the PBX for this user", theme.as_deref().unwrap_or("no choice")),
                Outcome::Taken(theme) => info!("theme: {} from the PBX", theme.as_deref().unwrap_or("no choice")),
                Outcome::Unreachable(why) => debug!("theme: not kept on the PBX now: {why}"),
                Outcome::InSync | Outcome::NoThemes => {}
            }
            // The UI and the others hear of a theme that was taken.  (A
            // write of the record alone changes nothing they use, and
            // telling them would only start another pass.)
            if let (Outcome::Taken(_), Some(cfg)) = (&outcome, &written) {
                hub.changed(&state, cfg);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendeskphone_apps::services::NoStore;
    use opendeskphone_apps::testutil::FakeStore;
    use opendeskphone_config::{ConfigStore, Model, Nvdata};
    use std::sync::Mutex;

    /// A phone: its nvdata in a directory of its own.
    struct Phone {
        _dir: tempfile::TempDir,
        config: SharedConfig,
    }

    fn phone_of(model: Model) -> Phone {
        let dir = tempfile::tempdir().unwrap();
        let nv = Nvdata::new(dir.path().join("nvdata"));
        nv.ensure_layout().unwrap();
        let store = ConfigStore::open(nv, model, dir.path().join("tmp")).unwrap();
        Phone { _dir: dir, config: Arc::new(Mutex::new(store)) }
    }

    fn phone() -> Phone {
        phone_of(Model::D60)
    }

    impl Phone {
        /// The user chooses a theme (the menu and the web UI do this).
        fn choose(&self, theme: Option<&str>) {
            let theme = theme.map(str::to_string);
            self.config.lock().unwrap().update_opendeskphone_settings(|s| s.theme = theme).unwrap();
        }
        fn theme(&self) -> Option<String> {
            self.config.lock().unwrap().config().opendeskphone_settings().local_theme()
        }
        /// The theme in effect ("" is the default one).
        fn shown(&self) -> String {
            match self.config.lock().unwrap().config().theme().choice {
                opendeskphone_config::ThemeChoice::Builtin(name) => name,
                opendeskphone_config::ThemeChoice::Picture => "(picture)".into(),
            }
        }
        fn record(&self) -> ThemeSync {
            local(&self.config).2
        }
        async fn sync(&self, store: &dyn PbxStore, now: Option<u64>) -> Outcome {
            sync(&self.config, store, now).await.0
        }
    }

    /// What the PBX has for the user of `store`.
    fn on_pbx(store: &FakeStore) -> Option<String> {
        match store.run(StoreOp::Get { app: PHONE_DATA_NAME.into(), scope: StoreScope::User, key: KEY.into() }) {
            StoreReply::Entry(e) => e.map(|e| String::from_utf8(e.value).unwrap()),
            other => panic!("{other:?}"),
        }
    }

    fn sent(theme: &str) -> Outcome {
        Outcome::Sent(Some(theme.into()))
    }

    fn taken(theme: &str) -> Outcome {
        Outcome::Taken(Some(theme.into()))
    }

    /// The writes a phone made, without its reads.
    fn writes(store: &FakeStore) -> usize {
        store.ops().iter().filter(|op| matches!(op, StoreOp::Set { .. } | StoreOp::Remove { .. })).count()
    }

    #[tokio::test]
    async fn first_use() {
        let pbx = FakeStore::default();
        let p = phone();
        // Nothing chosen and nothing on the PBX: nothing is written there.
        assert_eq!(p.sync(&pbx, Some(1000)).await, Outcome::InSync);
        assert_eq!((on_pbx(&pbx), writes(&pbx)), (None, 0));
        assert_eq!(p.record(), ThemeSync { user: "100".into(), ..Default::default() });
        // A theme is chosen: it is in effect at once, and the next pass
        // sends it, in the user's keys of the phone's name.
        p.choose(Some("Dusk"));
        assert_eq!(p.shown(), "dusk");
        assert_eq!(p.sync(&pbx, Some(1010)).await, sent("dusk"));
        assert_eq!(on_pbx(&pbx).as_deref(), Some("dusk"));
        assert!(pbx.ops().iter().any(|op| matches!(
            op,
            StoreOp::Set { app, scope: StoreScope::User, key, expect: Some(0), .. } if app == "opendeskphone" && key == "theme"
        )));
        let record = p.record();
        assert_eq!((record.user.as_str(), record.theme.as_deref(), record.changed_unix), ("100", Some("dusk"), 0));
        assert!(record.version > 0);
        // Passes after that have nothing to do.
        assert_eq!(p.sync(&pbx, Some(1020)).await, Outcome::InSync);
        assert_eq!(writes(&pbx), 1);

        // A phone that had a theme before it first reached the store (it
        // was chosen under a firmware that did not send it) sends it.
        let pbx = FakeStore::default();
        let p = phone();
        p.choose(Some("slate"));
        assert_eq!(p.sync(&pbx, None).await, sent("slate"));
        assert_eq!(on_pbx(&pbx).as_deref(), Some("slate"));
    }

    #[tokio::test]
    async fn a_reset_phone_and_another_phone_of_the_user() {
        let pbx = FakeStore::default();
        let p = phone();
        p.choose(Some("regatta"));
        assert_eq!(p.sync(&pbx, Some(1000)).await, sent("regatta"));
        // The phone is reset: its settings are gone, and its theme is the
        // default one.  The first pass brings the user's theme back.
        let reset = phone();
        assert_eq!((reset.theme(), reset.shown().as_str()), (None, ""));
        assert_eq!(reset.sync(&pbx, Some(2000)).await, taken("regatta"));
        assert_eq!((reset.theme().as_deref(), reset.shown().as_str()), (Some("regatta"), "regatta"));
        assert_eq!(writes(&pbx), 1, "taking writes nothing to the PBX");
        assert_eq!(reset.sync(&pbx, Some(2010)).await, Outcome::InSync);

        // Another phone of the same user takes it too, and a choice made
        // there reaches the first phone with its next pass.
        let (desk, desk_pbx) = (phone(), pbx.phone("100", "Desk"));
        assert_eq!(desk.sync(&desk_pbx, Some(3000)).await, taken("regatta"));
        desk.choose(Some("orbit"));
        assert_eq!(desk.sync(&desk_pbx, Some(3010)).await, sent("orbit"));
        assert_eq!(reset.sync(&pbx, Some(3020)).await, taken("orbit"));
        assert_eq!(reset.shown(), "orbit");
        // The choice dropped (the web UI's empty choice): the entry is
        // removed, and the other phone drops its choice as well.
        desk.choose(None);
        assert_eq!(desk.sync(&desk_pbx, Some(3030)).await, Outcome::Sent(None));
        assert_eq!(on_pbx(&pbx), None);
        assert_eq!(reset.sync(&pbx, Some(3040)).await, Outcome::Taken(None));
        assert_eq!((reset.theme(), reset.shown().as_str()), (None, ""));

        // Another user's phone has nothing to do with it.
        desk.choose(Some("fizz"));
        assert_eq!(desk.sync(&desk_pbx, Some(3050)).await, sent("fizz"));
        let (other, other_pbx) = (phone(), pbx.phone("200", "Lobby"));
        assert_eq!(other.sync(&other_pbx, Some(4000)).await, Outcome::InSync);
        assert_eq!((other.theme(), on_pbx(&other_pbx)), (None, None));
    }

    #[tokio::test]
    async fn the_pbx_out_of_reach_and_then_in_reach() {
        let pbx = FakeStore::default();
        let p = phone();
        p.choose(Some("dusk"));
        assert_eq!(p.sync(&pbx, Some(1000)).await, sent("dusk"));
        // Out of reach: a choice is the phone's setting all the same.
        pbx.set_online(false);
        p.choose(Some("tides"));
        assert!(matches!(p.sync(&pbx, Some(2000)).await, Outcome::Unreachable(_)));
        assert_eq!(p.shown(), "tides");
        // The record says when the choice was first seen, and is still
        // about the theme the PBX has.
        let record = p.record();
        assert_eq!((record.theme.as_deref(), record.changed_unix), (Some("dusk"), 2000));
        // Chosen again, still out of reach: the time stays the first.
        p.choose(Some("bokeh"));
        assert!(matches!(p.sync(&pbx, Some(2500)).await, Outcome::Unreachable(_)));
        assert_eq!(p.record().changed_unix, 2000);
        assert_eq!(writes(&pbx), 1);
        // In reach again: the last choice is sent.
        pbx.set_online(true);
        assert_eq!(p.sync(&pbx, Some(3000)).await, sent("bokeh"));
        assert_eq!(on_pbx(&pbx).as_deref(), Some("bokeh"));
        assert_eq!(p.record().changed_unix, 0);

        // Chosen and then put back while out of reach: nothing to send.
        pbx.set_online(false);
        p.choose(Some("orbit"));
        assert!(matches!(p.sync(&pbx, Some(4000)).await, Outcome::Unreachable(_)));
        p.choose(Some("bokeh"));
        pbx.set_online(true);
        assert_eq!(p.sync(&pbx, Some(4100)).await, Outcome::InSync);
        assert_eq!((p.record().changed_unix, writes(&pbx)), (0, 2));

        // A PBX that keeps no such store (the official DPMA): the theme
        // is chosen and kept on the phone, as before.
        let alone = phone();
        alone.choose(Some("lanterns"));
        assert!(matches!(alone.sync(&NoStore, Some(1000)).await, Outcome::Unreachable(_)));
        assert!(matches!(alone.sync(&NoStore, Some(1100)).await, Outcome::Unreachable(_)));
        assert_eq!((alone.shown().as_str(), alone.record().changed_unix), ("lanterns", 1000));
    }

    #[tokio::test]
    async fn both_sides_changed() {
        // Two phones of one user, in step on "dusk".
        let setup = || async {
            let pbx = FakeStore::default();
            let (a, b, b_pbx) = (phone(), phone(), pbx.phone("100", "Desk"));
            pbx.set_time(1000);
            a.choose(Some("dusk"));
            assert_eq!(a.sync(&pbx, Some(1000)).await, sent("dusk"));
            assert_eq!(b.sync(&b_pbx, Some(1000)).await, taken("dusk"));
            (pbx, a, b, b_pbx)
        };

        // Phone A is cut off and its user chooses there (at 2000); later
        // (at 3000) the user chooses on phone B, which is in reach.  When
        // A is back, the later choice, B's, stays on both.
        let (pbx, a, b, b_pbx) = setup().await;
        pbx.set_online(false);
        a.choose(Some("tides"));
        assert!(matches!(a.sync(&pbx, Some(2000)).await, Outcome::Unreachable(_)));
        pbx.set_time(3000);
        b.choose(Some("orbit"));
        assert_eq!(b.sync(&b_pbx, Some(3000)).await, sent("orbit"));
        pbx.set_online(true);
        assert_eq!(a.sync(&pbx, Some(4000)).await, taken("orbit"));
        assert_eq!((a.shown().as_str(), on_pbx(&pbx).as_deref()), ("orbit", Some("orbit")));
        assert_eq!(b.sync(&b_pbx, Some(4100)).await, Outcome::InSync);

        // The other way around: B's choice (at 1500) is the earlier one,
        // A's (at 2000, cut off) the later.  A's stays on both.
        let (pbx, a, b, b_pbx) = setup().await;
        pbx.set_time(1500);
        b.choose(Some("orbit"));
        assert_eq!(b.sync(&b_pbx, Some(1500)).await, sent("orbit"));
        pbx.set_online(false);
        a.choose(Some("tides"));
        assert!(matches!(a.sync(&pbx, Some(2000)).await, Outcome::Unreachable(_)));
        pbx.set_online(true);
        pbx.set_time(4000);
        assert_eq!(a.sync(&pbx, Some(4000)).await, sent("tides"));
        assert_eq!(on_pbx(&pbx).as_deref(), Some("tides"));
        assert_eq!(b.sync(&b_pbx, Some(4100)).await, taken("tides"));

        // A phone that did not know the time when its user chose: its
        // choice stays.
        let (pbx, a, b, b_pbx) = setup().await;
        pbx.set_time(3000);
        b.choose(Some("orbit"));
        assert_eq!(b.sync(&b_pbx, Some(3000)).await, sent("orbit"));
        a.choose(Some("tides"));
        assert_eq!(a.sync(&pbx, None).await, sent("tides"));
        assert_eq!(b.sync(&b_pbx, Some(4100)).await, taken("tides"));

        // Both chose the same: nothing is sent, and they are in step.
        let (pbx, a, b, b_pbx) = setup().await;
        b.choose(Some("fizz"));
        assert_eq!(b.sync(&b_pbx, Some(1500)).await, sent("fizz"));
        a.choose(Some("fizz"));
        assert_eq!(a.sync(&pbx, Some(900)).await, Outcome::InSync);
        assert_eq!(a.record().theme.as_deref(), Some("fizz"));
        assert_eq!(a.sync(&pbx, Some(1600)).await, Outcome::InSync);
        assert_eq!(writes(&pbx), 1);

        // A reset phone whose user chooses before the store is first in
        // reach: that choice is the latest, and it stays.
        let (pbx, _a, b, b_pbx) = setup().await;
        let reset = phone();
        reset.choose(Some("lanterns"));
        pbx.set_time(5000);
        assert_eq!(reset.sync(&pbx, Some(5000)).await, sent("lanterns"));
        assert_eq!(b.sync(&b_pbx, Some(5100)).await, taken("lanterns"));
    }

    #[tokio::test]
    async fn another_user_on_the_phone() {
        let pbx = FakeStore::default();
        let p = phone();
        p.choose(Some("dusk"));
        assert_eq!(p.sync(&pbx, Some(1000)).await, sent("dusk"));
        // The phone is provisioned for another user, who has no theme on
        // the PBX: the phone's theme stays, and is not sent as theirs.
        let lobby = pbx.phone("200", "Lobby");
        assert_eq!(p.sync(&lobby, Some(2000)).await, Outcome::InSync);
        assert_eq!((p.shown().as_str(), on_pbx(&lobby), writes(&lobby)), ("dusk", None, 0));
        assert_eq!(p.record().user, "200");
        // What they choose here is theirs.
        p.choose(Some("mist"));
        assert_eq!(p.sync(&lobby, Some(2100)).await, sent("mist"));
        assert_eq!((on_pbx(&lobby).as_deref(), on_pbx(&pbx).as_deref()), (Some("mist"), Some("dusk")));
        // The first user is back: their theme is taken from the PBX.
        assert_eq!(p.sync(&pbx, Some(3000)).await, taken("dusk"));
        assert_eq!(p.shown(), "dusk");
    }

    #[tokio::test]
    async fn what_is_not_a_theme_and_what_has_no_themes() {
        // An entry that is not a theme's name is not taken; a choice on
        // the phone replaces it.
        let pbx = FakeStore::default();
        let junk = |value: &[u8]| {
            pbx.run(StoreOp::Set { app: PHONE_DATA_NAME.into(), scope: StoreScope::User, key: KEY.into(), value: value.to_vec(), expect: None })
        };
        junk(b"Not A Theme!");
        let p = phone();
        assert_eq!(p.sync(&pbx, Some(1000)).await, Outcome::InSync);
        assert_eq!(p.theme(), None);
        junk(&[0xFF, 0xFE]);
        assert_eq!(p.sync(&pbx, Some(1100)).await, Outcome::InSync);
        assert_eq!(p.theme(), None);
        p.choose(Some("dusk"));
        assert_eq!(p.sync(&pbx, Some(1200)).await, sent("dusk"));
        assert_eq!(on_pbx(&pbx).as_deref(), Some("dusk"));
        // A theme of a newer firmware is a name like any other: it is
        // kept (the phone shows its default theme for it).
        junk(b"aurora-2");
        assert_eq!(p.sync(&pbx, Some(1300)).await, taken("aurora-2"));
        assert!(valid("hot-dog-stand") && !valid("") && !valid("Dusk") && !valid(&"a".repeat(33)));

        // A gray model has no themes: nothing is asked or written.
        let gray = phone_of(Model::D50);
        let pbx = FakeStore::default();
        gray.choose(Some("dusk"));
        assert_eq!(gray.sync(&pbx, Some(1000)).await, Outcome::NoThemes);
        assert!(pbx.ops().is_empty());
        assert_eq!(gray.record(), ThemeSync::default());
    }
}
