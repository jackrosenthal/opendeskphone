//! `AppsService`, `FirmwareService`, `LogsService`, `ScreenService`, and
//! `SystemService`.

use super::ssh::{locked, setting_source};
use super::{command, config_error, watch, with_config};
use crate::rpc::{Code, ConnectError, Sink};
use crate::http::Ctx;
use crate::{convert, pb, App, MirrorScreen};
use opendeskphone_state::{self as st, Command, CommandError, Key, Topic};
use std::sync::Arc;
use std::time::Duration;

// ------------------------------------------------------------------ apps

fn apps_snapshot(app: &App) -> pb::AppsSnapshot {
    convert::apps(&app.store.get::<st::Apps>())
}

pub async fn list_apps(ctx: Ctx, _req: pb::ListAppsRequest) -> Result<pb::ListAppsResponse, ConnectError> {
    Ok(pb::ListAppsResponse { snapshot: Some(apps_snapshot(&ctx.app)).into() })
}

pub async fn watch_apps(
    ctx: Ctx,
    _req: pb::WatchAppsRequest,
    sink: Sink<pb::WatchAppsResponse>,
) -> Result<(), ConnectError> {
    watch(ctx, Topic::Apps, sink, false, |app| pb::WatchAppsResponse { snapshot: Some(apps_snapshot(app)).into() }).await
}

pub async fn launch_app(ctx: Ctx, req: pb::LaunchAppRequest) -> Result<pb::LaunchAppResponse, ConnectError> {
    command(&ctx.app.store, Command::LaunchApp { name: req.name }).await?;
    Ok(pb::LaunchAppResponse {})
}

pub async fn close_app(ctx: Ctx, req: pb::CloseAppRequest) -> Result<pb::CloseAppResponse, ConnectError> {
    command(&ctx.app.store, Command::CloseApp { name: req.name }).await?;
    Ok(pb::CloseAppResponse {})
}

/// How long installing an app may take: the compilation of the largest
/// component the phone accepts, behind another one.
const INSTALL_APP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

pub async fn install_app(ctx: Ctx, req: pb::InstallAppRequest) -> Result<pb::InstallAppResponse, ConnectError> {
    let upload = {
        let mut uploads = ctx.app.uploads.lock().unwrap();
        match uploads.get(&req.upload_id) {
            Some(u) if u.firmware.is_none() => uploads.remove(&req.upload_id),
            _ => None,
        }
    }
    .ok_or_else(|| ConnectError::not_found("Upload expired.  Upload the file again."))?;
    let path = upload.path.to_string_lossy().into_owned();
    // The phone compiles the app, which takes from seconds to a few minutes
    // (`opendeskphone_apps::MAX_COMPONENT`), far longer than other commands.
    let sent = ctx.app.store.send(Command::InstallApp { path });
    let result = match tokio::time::timeout(INSTALL_APP_TIMEOUT, sent).await {
        Ok(r) => r.map_err(super::command_error),
        Err(_) => Err(super::command_error(opendeskphone_state::CommandError::Timeout)),
    };
    let _ = std::fs::remove_file(&upload.path);
    result?;
    Ok(pb::InstallAppResponse {})
}

pub async fn remove_app(ctx: Ctx, req: pb::RemoveAppRequest) -> Result<pb::RemoveAppResponse, ConnectError> {
    command(&ctx.app.store, Command::RemoveApp { name: req.name }).await?;
    Ok(pb::RemoveAppResponse {})
}

fn disabled_apps(r: opendeskphone_config::Resolved<Vec<String>>) -> pb::DisabledApps {
    pb::DisabledApps {
        apps: r.value.into_iter().map(|name| crate::proto::opendeskphone::settings::v1::App { name }).collect(),
        source: setting_source(r.source).into(),
        locked: r.locked,
    }
}

pub async fn get_disabled_apps(
    ctx: Ctx,
    _req: pb::GetDisabledAppsRequest,
) -> Result<pb::GetDisabledAppsResponse, ConnectError> {
    let setting = with_config(&ctx.app, false, |c| Ok(disabled_apps(c.config().disabled_apps()))).await?;
    Ok(pb::GetDisabledAppsResponse { disabled_apps: Some(setting).into() })
}

pub async fn set_app_enabled(
    ctx: Ctx,
    req: pb::SetAppEnabledRequest,
) -> Result<pb::SetAppEnabledResponse, ConnectError> {
    let apps = ctx.app.store.get::<st::Apps>();
    if !apps.installed.iter().chain(&apps.disabled).any(|a| a.builtin && a.name == req.name) {
        return Err(ConnectError::not_found("no built-in app of that name"));
    }
    let (name, enabled) = (req.name.clone(), req.enabled);
    let setting = with_config(&ctx.app, true, move |c| {
        let now = c.config().disabled_apps();
        if now.locked {
            return Err(locked());
        }
        let mut names = now.value;
        names.retain(|n| *n != name);
        if !enabled {
            names.push(name);
        }
        // With nothing switched off and nothing provisioned, the setting
        // goes back to its default; an empty list overrides a provisioned
        // one.
        let provisioned = c.config().setting(opendeskphone_config::settings::PROVISIONED_DISABLED_APPS).is_some();
        let list = (!names.is_empty() || provisioned).then_some(names);
        let config = c.update_opendeskphone_settings(|s| s.set_disabled_apps(list.as_deref())).map_err(config_error)?;
        Ok(disabled_apps(config.disabled_apps()))
    })
    .await?;
    log::info!("app {} switched {} from the web UI ({})", req.name, if enabled { "on" } else { "off" }, ctx.peer);
    Ok(pb::SetAppEnabledResponse { disabled_apps: Some(setting).into() })
}

// -------------------------------------------------------------- firmware

pub async fn get_firmware(ctx: Ctx, _req: pb::GetFirmwareRequest) -> Result<pb::GetFirmwareResponse, ConnectError> {
    Ok(pb::GetFirmwareResponse { status: Some(super::status::firmware_status(&ctx.app)).into() })
}

pub async fn watch_firmware(
    ctx: Ctx,
    _req: pb::WatchFirmwareRequest,
    sink: Sink<pb::WatchFirmwareResponse>,
) -> Result<(), ConnectError> {
    watch(ctx, Topic::Update, sink, false, |app| pb::WatchFirmwareResponse {
        status: Some(super::status::firmware_status(app)).into(),
    })
    .await
}

pub async fn install_firmware(
    ctx: Ctx,
    req: pb::InstallFirmwareRequest,
) -> Result<pb::InstallFirmwareResponse, ConnectError> {
    let model = ctx.app.store.model();
    let (path, is_stock) = {
        let uploads = ctx.app.uploads.lock().unwrap();
        let u = uploads
            .get(&req.upload_id)
            .filter(|u| u.firmware.is_some())
            .ok_or_else(|| ConnectError::not_found("Upload expired.  Upload the file again."))?;
        let info = u.firmware.as_ref().expect("filtered");
        if !info.model.is_empty() && !info.model.eq_ignore_ascii_case(model.name()) {
            return Err(ConnectError::failed_precondition(format!(
                "this image is for the {}, and this phone is a {}",
                info.model,
                model.name()
            )));
        }
        if info.is_stock && !req.allow_stock {
            return Err(ConnectError::new(
                Code::PermissionDenied,
                "this is a stock Digium image; installing it replaces Open Desk Phone. Confirm to proceed.",
            ));
        }
        (u.path.to_string_lossy().into_owned(), info.is_stock)
    };
    log::warn!(
        "firmware install requested from {} ({})",
        ctx.peer,
        if is_stock { "stock image" } else { "Open Desk Phone image" }
    );
    let sent = ctx.app.store.send(Command::InstallFirmware { path, allow_stock: req.allow_stock });
    // The updater may answer only when the flash is written, which takes
    // minutes; progress then shows through WatchFirmware.
    match tokio::time::timeout(Duration::from_secs(5), sent).await {
        Ok(Ok(_)) | Err(_) => {}
        Ok(Err(e @ CommandError::NoHandler(_))) => return Err(super::command_error(e)),
        Ok(Err(e)) => {
            return Err(super::command_error(e));
        }
    }
    // The image now belongs to the updater.
    ctx.app.uploads.lock().unwrap().remove(&req.upload_id);
    Ok(pb::InstallFirmwareResponse {})
}

pub async fn discard_upload(ctx: Ctx, req: pb::DiscardUploadRequest) -> Result<pb::DiscardUploadResponse, ConnectError> {
    if let Some(u) = ctx.app.uploads.lock().unwrap().remove(&req.upload_id) {
        if u.firmware.is_some() {
            ctx.app.firmware.discard(&u.path);
        } else {
            let _ = std::fs::remove_file(&u.path);
        }
    }
    Ok(pb::DiscardUploadResponse {})
}

pub async fn confirm_trial(ctx: Ctx, _req: pb::ConfirmTrialRequest) -> Result<pb::ConfirmTrialResponse, ConnectError> {
    command(&ctx.app.store, Command::ConfirmTrial).await?;
    Ok(pb::ConfirmTrialResponse {})
}

pub async fn rollback(ctx: Ctx, _req: pb::RollbackRequest) -> Result<pb::RollbackResponse, ConnectError> {
    log::warn!("firmware rollback requested from {}", ctx.peer);
    command(&ctx.app.store, Command::Rollback).await?;
    Ok(pb::RollbackResponse {})
}

// ------------------------------------------------------------------ logs

fn level_from(l: buffa::EnumValue<pb::LogLevel>) -> st::Level {
    match l.as_known().unwrap_or(pb::LogLevel::Unspecified) {
        pb::LogLevel::Error => st::Level::Error,
        pb::LogLevel::Warn => st::Level::Warn,
        pb::LogLevel::Info => st::Level::Info,
        pb::LogLevel::Debug => st::Level::Debug,
        pb::LogLevel::Trace | pb::LogLevel::Unspecified => st::Level::Trace,
    }
}

pub async fn get_logs(ctx: Ctx, req: pb::GetLogsRequest) -> Result<pb::GetLogsResponse, ConnectError> {
    let min = level_from(req.min_level);
    let mut entries: Vec<pb::LogEntry> = ctx
        .app
        .store
        .log()
        .since(req.after_seq)
        .iter()
        .filter(|e| e.level <= min)
        .map(convert::log_entry)
        .collect();
    if req.limit > 0 && entries.len() > req.limit as usize {
        entries.drain(..entries.len() - req.limit as usize);
    }
    Ok(pb::GetLogsResponse { entries })
}

/// Entries per stream message, so one message stays small.
const LOG_BATCH: usize = 200;

pub async fn tail_logs(
    ctx: Ctx,
    req: pb::TailLogsRequest,
    sink: Sink<pb::TailLogsResponse>,
) -> Result<(), ConnectError> {
    let app = ctx.app.clone();
    let token = ctx.session.as_ref().map(|s| s.token.clone());
    let min = level_from(req.min_level);
    let sub = app.store.subscribe(Topic::Log);
    let backlog = if req.backlog == 0 { 200 } else { req.backlog.min(5000) } as usize;
    let mut pending: Vec<st::LogEntry> = app.store.log().tail(backlog);
    let mut last = pending.last().map(|e| e.seq).unwrap_or_else(|| app.store.log().last_seq());
    // An empty first message tells the page the stream is up.
    let mut first = true;
    loop {
        let entries: Vec<pb::LogEntry> = pending.iter().filter(|e| e.level <= min).map(convert::log_entry).collect();
        if first && entries.is_empty() {
            sink.send(&pb::TailLogsResponse { entries: Vec::new() }).await?;
        }
        first = false;
        for batch in entries.chunks(LOG_BATCH) {
            sink.send(&pb::TailLogsResponse { entries: batch.to_vec() }).await?;
        }
        tokio::select! {
            _ = sub.changed() => tokio::time::sleep(Duration::from_millis(150)).await,
            _ = tokio::time::sleep(super::REFRESH) => {}
            _ = sink.closed() => return Ok(()),
        }
        let _ = sub.try_take();
        if let Some(t) = &token
            && app.sessions.touch(t).is_none() {
                return Err(ConnectError::new(Code::Unauthenticated, "session ended"));
            }
        pending = app.store.log().since(last);
        if let Some(e) = pending.last() {
            last = e.seq;
        }
    }
}

// ---------------------------------------------------------------- screen

/// The keys a model has.
fn model_keys(model: st::Model) -> Vec<pb::Key> {
    // The D80 has a touchscreen and no keys.
    if model.is_touch() {
        return Vec::new();
    }
    let mut keys: Vec<Key> = (1..=model.line_keys()).map(Key::Line).collect();
    keys.extend((1..=4).map(Key::Soft));
    keys.extend([Key::Up, Key::Down, Key::Left, Key::Right, Key::Select, Key::Cancel]);
    keys.extend((1..=9).map(Key::Digit));
    keys.extend([Key::Star, Key::Digit(0), Key::Pound]);
    keys.extend([
        Key::VolumeDown,
        Key::VolumeUp,
        Key::Mute,
        Key::Speaker,
        Key::Headset,
        Key::Hold,
        Key::Transfer,
        Key::Redial,
        Key::Messages,
    ]);
    if model.has_conference_key() {
        keys.extend([Key::Conference, Key::Info]);
    }
    keys.extend((1..=model.blf_keys()).map(Key::Blf));
    match model {
        st::Model::D70 => {
            keys.extend([Key::BlfPageLeft, Key::BlfPageRight, Key::Status, Key::Contacts, Key::Apps, Key::Menu])
        }
        st::Model::D65 => keys.extend([Key::BlfPageLeft, Key::BlfPageRight]),
        _ => {}
    }
    keys.into_iter().map(convert::key).collect()
}

pub async fn get_screen_layout(
    ctx: Ctx,
    _req: pb::GetScreenLayoutRequest,
) -> Result<pb::GetScreenLayoutResponse, ConnectError> {
    let model = ctx.app.store.model();
    let main = model.main_screen();
    let mut screens = vec![pb::ScreenInfo {
        id: pb::ScreenId::Main.into(),
        width: main.width as u32,
        height: main.height as u32,
        color: model.is_color(),
    }];
    if let Some(side) = model.side_screen() {
        screens.push(pb::ScreenInfo {
            id: pb::ScreenId::Side.into(),
            width: side.width as u32,
            height: side.height as u32,
            color: false,
        });
    }
    Ok(pb::GetScreenLayoutResponse {
        model: convert::model(model).into(),
        skin: convert::skin(model).into(),
        screens,
        keys: model_keys(model),
        softkey_bar_height: model.softkey_bar_height() as u32,
        mirror_available: ctx.app.screen.is_some(),
    })
}

fn lamps(app: &App) -> Vec<pb::Lamp> {
    let keys = app.store.get::<st::Keys>();
    let mut out = Vec::new();
    let mut add = |key: Key, lamp: st::LampState| {
        out.push(pb::Lamp {
            key: Some(convert::key(key)).into(),
            color: convert::lamp_color(lamp.color).into(),
            blink: convert::lamp_blink(lamp.blink).into(),
        })
    };
    for k in &keys.line_keys {
        add(Key::Line(k.index + 1), k.lamp);
    }
    for k in keys.rapid_dial.iter().filter(|k| k.page == keys.rapid_page) {
        add(Key::Blf(k.index + 1), k.lamp);
    }
    add(Key::Messages, keys.message_lamp);
    add(Key::Mute, keys.mute_lamp);
    add(Key::Headset, keys.headset_lamp);
    add(Key::Speaker, keys.speaker_lamp);
    out
}

pub async fn watch_screen(
    ctx: Ctx,
    req: pb::WatchScreenRequest,
    sink: Sink<pb::WatchScreenResponse>,
) -> Result<(), ConnectError> {
    let app = ctx.app.clone();
    let token = ctx.session.as_ref().map(|s| s.token.clone());
    let which = match req.screen.as_known().unwrap_or(pb::ScreenId::Main) {
        pb::ScreenId::Side => MirrorScreen::Side,
        _ => MirrorScreen::Main,
    };
    if which == MirrorScreen::Side && app.store.model().side_screen().is_none() {
        return Err(ConnectError::not_found("this model has no side display"));
    }
    let fps = if req.max_fps == 0 { 5 } else { req.max_fps.clamp(1, 15) };
    let period = Duration::from_millis(1000 / fps as u64);
    let sub = app.store.subscribe([Topic::Keys, Topic::Ui, Topic::Audio]);
    let mut last_gen: Option<u64> = None;
    let mut last_meta: Option<(Vec<pb::Lamp>, bool, bool)> = None;
    let mut since_touch = std::time::Instant::now();
    loop {
        let meta = (lamps(&app), app.store.get::<st::Ui>().backlight_on, app.store.get::<st::Audio>().off_hook);
        let generation = app.screen.as_ref().map(|s| s.generation());
        let picture_changed = generation.is_some() && generation != last_gen;
        if picture_changed || last_meta.as_ref() != Some(&meta) || last_meta.is_none() {
            let mut msg = pb::WatchScreenResponse {
                png: Vec::new(),
                width: 0,
                height: 0,
                lamps: meta.0.clone(),
                backlight_on: meta.1,
                off_hook: meta.2,
            };
            if picture_changed {
                let source = app.screen.clone().expect("generation came from it");
                // Capture and compression are too slow for the reactor
                // thread on the phone.
                let encoded = tokio::task::spawn_blocking(move || {
                    let frame = source.capture(which)?;
                    Some((frame.width, frame.height, crate::png::encode(&frame)?))
                })
                .await
                .ok()
                .flatten();
                if let Some((w, h, png)) = encoded {
                    msg.png = png;
                    msg.width = w as u32;
                    msg.height = h as u32;
                }
                last_gen = generation;
            }
            sink.send(&msg).await?;
            last_meta = Some(meta);
        }
        tokio::select! {
            _ = tokio::time::sleep(period) => {}
            _ = sink.closed() => return Ok(()),
        }
        let _ = sub.try_take();
        if since_touch.elapsed() > Duration::from_secs(15) {
            since_touch = std::time::Instant::now();
            if let Some(t) = &token
                && app.sessions.touch(t).is_none() {
                    return Err(ConnectError::new(Code::Unauthenticated, "session ended"));
                }
        }
    }
}

pub async fn press_key(ctx: Ctx, req: pb::PressKeyRequest) -> Result<pb::PressKeyResponse, ConnectError> {
    let key = req.key.as_option().and_then(convert::key_from).ok_or_else(|| ConnectError::invalid_argument("no such key"))?;
    let store = &ctx.app.store;
    match req.action.as_known().unwrap_or(pb::KeyAction::Tap) {
        pb::KeyAction::Down => {
            command(store, Command::InjectKey { key, down: true }).await?;
        }
        pb::KeyAction::Up => {
            command(store, Command::InjectKey { key, down: false }).await?;
        }
        pb::KeyAction::Tap | pb::KeyAction::Unspecified => {
            command(store, Command::InjectKey { key, down: true }).await?;
            tokio::time::sleep(Duration::from_millis(60)).await;
            command(store, Command::InjectKey { key, down: false }).await?;
        }
    }
    Ok(pb::PressKeyResponse {})
}

pub async fn set_hook(ctx: Ctx, req: pb::SetHookRequest) -> Result<pb::SetHookResponse, ConnectError> {
    command(&ctx.app.store, Command::InjectHook { off_hook: req.off_hook }).await?;
    Ok(pb::SetHookResponse {})
}

// ---------------------------------------------------------------- system

pub async fn reboot(ctx: Ctx, _req: pb::RebootRequest) -> Result<pb::RebootResponse, ConnectError> {
    log::warn!("reboot requested from {}", ctx.peer);
    command(&ctx.app.store, Command::Reboot { reason: "web interface".into() }).await?;
    Ok(pb::RebootResponse {})
}

pub async fn factory_reset(ctx: Ctx, req: pb::FactoryResetRequest) -> Result<pb::FactoryResetResponse, ConnectError> {
    let model = ctx.app.store.model();
    if !req.confirm.trim().eq_ignore_ascii_case(model.name()) {
        return Err(ConnectError::invalid_argument(format!("type {} to confirm the factory reset", model.name())));
    }
    log::warn!("factory reset requested from {}", ctx.peer);
    command(&ctx.app.store, Command::FactoryReset).await?;
    Ok(pb::FactoryResetResponse {})
}

pub async fn erase_nvdata(ctx: Ctx, req: pb::EraseNvdataRequest) -> Result<pb::EraseNvdataResponse, ConnectError> {
    let model = ctx.app.store.model();
    if !req.confirm.trim().eq_ignore_ascii_case(model.name()) {
        return Err(ConnectError::invalid_argument(format!("type {} to confirm erasing nvdata", model.name())));
    }
    log::warn!("erasing nvdata requested from {}", ctx.peer);
    command(&ctx.app.store, Command::EraseNvdata).await?;
    Ok(pb::EraseNvdataResponse {})
}

pub async fn set_time(ctx: Ctx, req: pb::SetTimeRequest) -> Result<pb::SetTimeResponse, ConnectError> {
    // 2020-01-01 to 2100-01-01: anything else is a mistake.
    if !(1_577_836_800..4_102_444_800).contains(&req.unix_secs) {
        return Err(ConnectError::invalid_argument("Invalid time."));
    }
    command(&ctx.app.store, Command::SetTime { unix_secs: req.unix_secs }).await?;
    Ok(pb::SetTimeResponse {})
}

async fn blocking<R: Send + 'static>(
    app: &Arc<App>,
    f: impl FnOnce(&App) -> std::io::Result<R> + Send + 'static,
) -> Result<std::io::Result<R>, ConnectError> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || f(&app))
        .await
        .map_err(|e| ConnectError::internal(format!("task failed: {e}")))
}

pub async fn create_backup(ctx: Ctx, _req: pb::CreateBackupRequest) -> Result<pb::CreateBackupResponse, ConnectError> {
    let archive = blocking(&ctx.app, |app| {
        // Hold the configuration lock so a write cannot interleave.
        let _g = app.config.lock().unwrap_or_else(|p| p.into_inner());
        app.backup.export()
    })
    .await?
    .map_err(|e| ConnectError::internal(format!("Backup failed: {e}")))?;
    let sys = ctx.app.store.get::<st::System>();
    let mac: String = sys.mac_string().replace(':', "");
    let filename = format!(
        "opendeskphone-{}{}{}-backup.tar.gz",
        sys.model.name().to_lowercase(),
        if mac.is_empty() { "" } else { "-" },
        mac
    );
    Ok(pb::CreateBackupResponse { filename, archive })
}

pub async fn restore_backup(ctx: Ctx, req: pb::RestoreBackupRequest) -> Result<pb::RestoreBackupResponse, ConnectError> {
    if req.archive.is_empty() {
        return Err(ConnectError::invalid_argument("No backup file."));
    }
    let archive = req.archive;
    let files = blocking(&ctx.app, move |app| {
        let mut g = app.config.lock().unwrap_or_else(|p| p.into_inner());
        let n = app.backup.import(&archive)?;
        // Merge what was just written.
        g.reload().map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(n)
    })
    .await?
    .map_err(|e| ConnectError::invalid_argument(format!("Restore failed: {e}")))?;
    log::warn!("configuration restored from a backup ({files} files) by {}", ctx.peer);
    ctx.app.store.post(Command::ReloadConfig);
    ctx.app.store.post(Command::ReloadContacts);
    Ok(pb::RestoreBackupResponse { files, restart_needed: true })
}
