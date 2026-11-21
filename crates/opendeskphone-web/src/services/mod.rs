//! The Connect services.  [`router`] registers every handler.

// The generated traits declare `impl Encodable<Res>` return types; the
// impls here name the concrete message, as the connectrpc guide directs.
#![allow(refining_impl_trait_internal, refining_impl_trait_reachable)]

mod alarms;
mod auth;
mod calls;
mod config;
mod devices;
mod mirror;
mod shell;
mod ssh;
mod status;
mod system;

use crate::rpc::{self, Code, ConnectError, Sink};
use connectrpc::{RequestContext, ServiceRequest, ServiceResult, ServiceStream};
use std::sync::Arc;
use crate::http::Ctx;
use crate::{pb, App, FirmwareInfo};
use opendeskphone_config::{ConfigError, ConfigStore};
use opendeskphone_state::{Command, CommandError, Reply, Store, TopicSet};
use std::time::Duration;

/// How long a command may take before the RPC gives up.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
/// Bursts of state changes within this window become one stream message.
const COALESCE: Duration = Duration::from_millis(120);
/// A watch stream re-sends at least this often (which also keeps the
/// session alive while a page is open, and gets past idle proxies).
const REFRESH: Duration = Duration::from_secs(20);

/// The receiver of every generated service trait.  Handlers are plain
/// functions; these impls only fetch the request context that the HTTP
/// layer attached and hand over the owned request message.
pub(crate) struct Rpc;

fn ctx_of(rc: &RequestContext) -> Result<Ctx, ConnectError> {
    rc.extensions().get::<Ctx>().cloned().ok_or_else(|| ConnectError::internal("request context missing"))
}

macro_rules! service {
    ($trait:ident { $( $kind:ident $method:ident($Req:ident) -> $Res:ident = $f:path; )* }) => {
        impl pb::$trait for Rpc { $( service!(@$kind $method $Req $Res $f); )* }
    };
    (@unary $method:ident $Req:ident $Res:ident $f:path) => {
        async fn $method(&self, rc: RequestContext, req: ServiceRequest<'_, pb::$Req>) -> ServiceResult<pb::$Res> {
            let ctx = ctx_of(&rc)?;
            let headers = ctx.headers.clone();
            let res = $f(ctx, req.to_owned_message()).await?;
            Ok(headers.apply(connectrpc::Response::new(res)))
        }
    };
    (@stream $method:ident $Req:ident $Res:ident $f:path) => {
        async fn $method(
            &self,
            rc: RequestContext,
            req: ServiceRequest<'_, pb::$Req>,
        ) -> ServiceResult<ServiceStream<pb::$Res>> {
            let ctx = ctx_of(&rc)?;
            let msg = req.to_owned_message();
            Ok(connectrpc::Response::new(rpc::stream(move |sink| $f(ctx, msg, sink))))
        }
    };
}

service!(AuthService {
    unary login(LoginRequest) -> LoginResponse = auth::login;
    unary logout(LogoutRequest) -> LogoutResponse = auth::logout;
    unary get_session(GetSessionRequest) -> GetSessionResponse = auth::get_session;
    unary change_password(ChangePasswordRequest) -> ChangePasswordResponse = auth::change_password;
});

service!(StatusService {
    unary get_status(GetStatusRequest) -> GetStatusResponse = status::get_status;
    stream watch_status(WatchStatusRequest) -> WatchStatusResponse = status::watch_status;
});

service!(NetworkService {
    unary get_network(GetNetworkRequest) -> GetNetworkResponse = status::get_network;
    stream watch_network(WatchNetworkRequest) -> WatchNetworkResponse = status::watch_network;
    unary set_network_settings(SetNetworkSettingsRequest) -> SetNetworkSettingsResponse = config::set_network_settings;
    unary renew_dhcp(RenewDhcpRequest) -> RenewDhcpResponse = status::renew_dhcp;
    unary sync_time(SyncTimeRequest) -> SyncTimeResponse = status::sync_time;
});

service!(ProvisioningService {
    unary get_provisioning(GetProvisioningRequest) -> GetProvisioningResponse = status::get_provisioning;
    stream watch_provisioning(WatchProvisioningRequest) -> WatchProvisioningResponse = status::watch_provisioning;
    unary start_provisioning(StartProvisioningRequest) -> StartProvisioningResponse = status::start_provisioning;
    unary select_user(SelectUserRequest) -> SelectUserResponse = status::select_user;
    unary reprovision(ReprovisionRequest) -> ReprovisionResponse = status::reprovision;
    unary forget_provisioning(ForgetProvisioningRequest) -> ForgetProvisioningResponse = status::forget_provisioning;
    unary skip_provisioning(SkipProvisioningRequest) -> SkipProvisioningResponse = status::skip_provisioning;
});

service!(AccountsService {
    unary list_accounts(ListAccountsRequest) -> ListAccountsResponse = config::list_accounts;
    stream watch_accounts(WatchAccountsRequest) -> WatchAccountsResponse = status::watch_accounts;
    unary set_accounts(SetAccountsRequest) -> SetAccountsResponse = config::set_accounts;
    unary clear_local_accounts(ClearLocalAccountsRequest) -> ClearLocalAccountsResponse = config::clear_local_accounts;
    unary reregister(ReregisterRequest) -> ReregisterResponse = status::reregister;
});

service!(ConfigService {
    unary list_settings(ListSettingsRequest) -> ListSettingsResponse = config::list_settings;
    unary set_settings(SetSettingsRequest) -> SetSettingsResponse = config::set_settings;
    unary get_idle_softkeys(GetIdleSoftkeysRequest) -> GetIdleSoftkeysResponse = config::get_idle_softkeys;
    unary set_idle_softkeys(SetIdleSoftkeysRequest) -> SetIdleSoftkeysResponse = config::set_idle_softkeys;
    unary get_theme(GetThemeRequest) -> GetThemeResponse = config::get_theme;
    unary set_theme(SetThemeRequest) -> SetThemeResponse = config::set_theme;
});

service!(ContactsService {
    unary list_contact_lists(ListContactListsRequest) -> ListContactListsResponse = config::list_contact_lists;
    unary get_contact_list(GetContactListRequest) -> GetContactListResponse = config::get_contact_list;
    unary put_contact_list(PutContactListRequest) -> PutContactListResponse = config::put_contact_list;
    unary delete_contact_list(DeleteContactListRequest) -> DeleteContactListResponse = config::delete_contact_list;
    unary export_contact_list(ExportContactListRequest) -> ExportContactListResponse = config::export_contact_list;
    unary import_contact_list(ImportContactListRequest) -> ImportContactListResponse = config::import_contact_list;
});

service!(CallsService {
    unary get_calls(GetCallsRequest) -> GetCallsResponse = calls::get_calls;
    stream watch_calls(WatchCallsRequest) -> WatchCallsResponse = calls::watch_calls;
    unary dial(DialRequest) -> DialResponse = calls::dial;
    unary answer(AnswerRequest) -> AnswerResponse = calls::answer;
    unary hangup(HangupRequest) -> HangupResponse = calls::hangup;
    unary hold(HoldRequest) -> HoldResponse = calls::hold;
    unary resume(ResumeRequest) -> ResumeResponse = calls::resume;
    unary transfer(TransferRequest) -> TransferResponse = calls::transfer;
    unary send_dtmf(SendDtmfRequest) -> SendDtmfResponse = calls::send_dtmf;
    unary set_mute(SetMuteRequest) -> SetMuteResponse = calls::set_mute;
    unary set_dnd(SetDndRequest) -> SetDndResponse = calls::set_dnd;
    unary set_forward(SetForwardRequest) -> SetForwardResponse = calls::set_forward;
    unary get_call_log(GetCallLogRequest) -> GetCallLogResponse = calls::get_call_log;
    unary clear_call_log(ClearCallLogRequest) -> ClearCallLogResponse = calls::clear_call_log;
});

service!(AppsService {
    unary list_apps(ListAppsRequest) -> ListAppsResponse = system::list_apps;
    stream watch_apps(WatchAppsRequest) -> WatchAppsResponse = system::watch_apps;
    unary launch_app(LaunchAppRequest) -> LaunchAppResponse = system::launch_app;
    unary close_app(CloseAppRequest) -> CloseAppResponse = system::close_app;
    unary install_app(InstallAppRequest) -> InstallAppResponse = system::install_app;
    unary remove_app(RemoveAppRequest) -> RemoveAppResponse = system::remove_app;
    unary get_disabled_apps(GetDisabledAppsRequest) -> GetDisabledAppsResponse = system::get_disabled_apps;
    unary set_app_enabled(SetAppEnabledRequest) -> SetAppEnabledResponse = system::set_app_enabled;
});

service!(FirmwareService {
    unary get_firmware(GetFirmwareRequest) -> GetFirmwareResponse = system::get_firmware;
    stream watch_firmware(WatchFirmwareRequest) -> WatchFirmwareResponse = system::watch_firmware;
    unary install_firmware(InstallFirmwareRequest) -> InstallFirmwareResponse = system::install_firmware;
    unary discard_upload(DiscardUploadRequest) -> DiscardUploadResponse = system::discard_upload;
    unary confirm_trial(ConfirmTrialRequest) -> ConfirmTrialResponse = system::confirm_trial;
    unary rollback(RollbackRequest) -> RollbackResponse = system::rollback;
    unary get_provisioned_firmware_policy(GetProvisionedFirmwarePolicyRequest) -> GetProvisionedFirmwarePolicyResponse = ssh::get_provisioned_firmware_policy;
    unary set_provisioned_firmware_policy(SetProvisionedFirmwarePolicyRequest) -> SetProvisionedFirmwarePolicyResponse = ssh::set_provisioned_firmware_policy;
});

service!(LogsService {
    unary get_logs(GetLogsRequest) -> GetLogsResponse = system::get_logs;
    stream tail_logs(TailLogsRequest) -> TailLogsResponse = system::tail_logs;
});

service!(ScreenService {
    unary get_screen_layout(GetScreenLayoutRequest) -> GetScreenLayoutResponse = system::get_screen_layout;
    stream watch_screen(WatchScreenRequest) -> WatchScreenResponse = system::watch_screen;
    unary press_key(PressKeyRequest) -> PressKeyResponse = system::press_key;
    unary touch(TouchRequest) -> TouchResponse = system::touch;
    unary set_hook(SetHookRequest) -> SetHookResponse = system::set_hook;
});

service!(SystemService {
    unary reboot(RebootRequest) -> RebootResponse = system::reboot;
    unary factory_reset(FactoryResetRequest) -> FactoryResetResponse = system::factory_reset;
    unary erase_nvdata(EraseNvdataRequest) -> EraseNvdataResponse = system::erase_nvdata;
    unary set_time(SetTimeRequest) -> SetTimeResponse = system::set_time;
    unary create_backup(CreateBackupRequest) -> CreateBackupResponse = system::create_backup;
    unary restore_backup(RestoreBackupRequest) -> RestoreBackupResponse = system::restore_backup;
    unary get_serial_console(GetSerialConsoleRequest) -> GetSerialConsoleResponse = ssh::get_serial_console;
    unary set_serial_console(SetSerialConsoleRequest) -> SetSerialConsoleResponse = ssh::set_serial_console;
    unary get_metrics_enabled(GetMetricsEnabledRequest) -> GetMetricsEnabledResponse = ssh::get_metrics_enabled;
    unary set_metrics_enabled(SetMetricsEnabledRequest) -> SetMetricsEnabledResponse = ssh::set_metrics_enabled;
});

service!(ShellService {
    stream open_shell(OpenShellRequest) -> OpenShellResponse = shell::open_shell;
    unary write_shell(WriteShellRequest) -> WriteShellResponse = shell::write_shell;
    unary resize_shell(ResizeShellRequest) -> ResizeShellResponse = shell::resize_shell;
    unary close_shell(CloseShellRequest) -> CloseShellResponse = shell::close_shell;
});

service!(SshService {
    unary get_ssh(GetSshRequest) -> GetSshResponse = ssh::get_ssh;
    unary set_ssh_enabled(SetSshEnabledRequest) -> SetSshEnabledResponse = ssh::set_ssh_enabled;
    unary add_ssh_key(AddSshKeyRequest) -> AddSshKeyResponse = ssh::add_ssh_key;
    unary remove_ssh_key(RemoveSshKeyRequest) -> RemoveSshKeyResponse = ssh::remove_ssh_key;
});

service!(BluetoothService {
    unary get_bluetooth(GetBluetoothRequest) -> GetBluetoothResponse = devices::get_bluetooth;
    stream watch_bluetooth(WatchBluetoothRequest) -> WatchBluetoothResponse = devices::watch_bluetooth;
    unary set_bluetooth_discovery(SetBluetoothDiscoveryRequest) -> SetBluetoothDiscoveryResponse = devices::set_bluetooth_discovery;
    unary pair_bluetooth_device(PairBluetoothDeviceRequest) -> PairBluetoothDeviceResponse = devices::pair_bluetooth_device;
    unary answer_bluetooth_pairing(AnswerBluetoothPairingRequest) -> AnswerBluetoothPairingResponse = devices::answer_bluetooth_pairing;
    unary remove_bluetooth_device(RemoveBluetoothDeviceRequest) -> RemoveBluetoothDeviceResponse = devices::remove_bluetooth_device;
    unary connect_bluetooth_device(ConnectBluetoothDeviceRequest) -> ConnectBluetoothDeviceResponse = devices::connect_bluetooth_device;
    unary disconnect_bluetooth_device(DisconnectBluetoothDeviceRequest) -> DisconnectBluetoothDeviceResponse = devices::disconnect_bluetooth_device;
});

service!(PagingService {
    unary get_paging(GetPagingRequest) -> GetPagingResponse = devices::get_paging;
    stream watch_paging(WatchPagingRequest) -> WatchPagingResponse = devices::watch_paging;
    unary start_page(StartPageRequest) -> StartPageResponse = devices::start_page;
    unary stop_page(StopPageRequest) -> StopPageResponse = devices::stop_page;
});

service!(ExpansionService {
    unary get_expansion(GetExpansionRequest) -> GetExpansionResponse = devices::get_expansion;
    stream watch_expansion(WatchExpansionRequest) -> WatchExpansionResponse = devices::watch_expansion;
});

service!(AlarmsService {
    unary get_alarms(GetAlarmsRequest) -> GetAlarmsResponse = alarms::get_alarms;
    stream watch_alarms(WatchAlarmsRequest) -> WatchAlarmsResponse = alarms::watch_alarms;
    unary set_alarm(SetAlarmRequest) -> SetAlarmResponse = alarms::set_alarm;
    unary remove_alarm(RemoveAlarmRequest) -> RemoveAlarmResponse = alarms::remove_alarm;
    unary snooze_alarm(SnoozeAlarmRequest) -> SnoozeAlarmResponse = alarms::snooze_alarm;
    unary dismiss_alarm(DismissAlarmRequest) -> DismissAlarmResponse = alarms::dismiss_alarm;
});

/// The Connect router with every service registered.
pub(crate) fn router() -> connectrpc::Router {
    let rpc = Arc::new(Rpc);
    connectrpc::Router::new()
        .add_service::<Rpc, pb::AuthServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::StatusServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::NetworkServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::ProvisioningServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::AccountsServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::ConfigServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::ContactsServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::CallsServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::AppsServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::FirmwareServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::LogsServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::ScreenServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::SystemServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::ShellServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::SshServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::BluetoothServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::PagingServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::ExpansionServiceRegisterMarker>(rpc.clone())
        .add_service::<Rpc, pb::AlarmsServiceRegisterMarker>(rpc.clone())
}

/// A command failure as an RPC error.
pub(crate) fn command_error(e: CommandError) -> ConnectError {
    {
        let code = match &e {
            CommandError::NoHandler(_) | CommandError::Dropped => Code::Unavailable,
            CommandError::Timeout => Code::DeadlineExceeded,
            CommandError::Invalid(_) => Code::FailedPrecondition,
            CommandError::Failed(_) => Code::Unknown,
            CommandError::Denied(_) => Code::PermissionDenied,
        };
        let message = match e {
            CommandError::Invalid(s) | CommandError::Failed(s) | CommandError::Denied(s) => s,
            CommandError::NoHandler(d) => format!("the {d:?} subsystem is not running"),
            other => other.to_string(),
        };
        ConnectError::new(code, message)
    }
}

/// A configuration failure as an RPC error.
pub(crate) fn config_error(e: ConfigError) -> ConnectError {
    {
        let code = match &e {
            ConfigError::Io(_) => Code::Internal,
            ConfigError::Parse(_) | ConfigError::NotAStockSetting(_) | ConfigError::TooManyAccounts { .. } => {
                Code::InvalidArgument
            }
            ConfigError::Frozen(_) | ConfigError::ReadOnlyLayer(_) => Code::FailedPrecondition,
        };
        ConnectError::new(code, e.to_string())
    }
}

/// Sends a command to its subsystem and waits for the answer.
pub(crate) async fn command(store: &Store, cmd: Command) -> Result<Reply, ConnectError> {
    match tokio::time::timeout(COMMAND_TIMEOUT, store.send(cmd)).await {
        Ok(r) => r.map_err(command_error),
        Err(_) => Err(command_error(CommandError::Timeout)),
    }
}

/// Runs a configuration operation off the reactor thread (writes fsync on
/// flash).  After a write, pass `changed = true` so the configuration
/// owner republishes.
pub(crate) async fn with_config<R, F>(app: &std::sync::Arc<App>, changed: bool, f: F) -> Result<R, ConnectError>
where
    R: Send + 'static,
    F: FnOnce(&mut ConfigStore) -> Result<R, ConnectError> + Send + 'static,
{
    let config = app.config.clone();
    let out = tokio::task::spawn_blocking(move || {
        let mut g = config.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut g)
    })
    .await
    .map_err(|e| ConnectError::internal(format!("configuration task failed: {e}")))??;
    if changed {
        // Nobody handling Config commands (the simulator, early boot) is
        // fine: the write itself has already been merged.
        app.store.post(Command::ReloadConfig);
    }
    Ok(out)
}

/// The body of every "watch" RPC: send a snapshot now, then again when a
/// subscribed topic changes and the snapshot differs.
pub(crate) async fn watch<Res, F>(
    ctx: Ctx,
    topics: impl Into<TopicSet>,
    sink: Sink<Res>,
    always_refresh: bool,
    make: F,
) -> Result<(), ConnectError>
where
    Res: Clone + PartialEq + Send + 'static,
    F: Fn(&App) -> Res + Send + 'static,
{
    let app = ctx.app.clone();
    let token = ctx.session.as_ref().map(|s| s.token.clone());
    let sub = app.store.subscribe(topics);
    let mut last: Option<Res> = None;
    loop {
        if let Some(t) = &token
            && app.sessions.touch(t).is_none() {
                return Err(ConnectError::new(Code::Unauthenticated, "session ended"));
            }
        let msg = make(&app);
        if always_refresh || last.as_ref() != Some(&msg) {
            sink.send(&msg).await?;
            last = Some(msg);
        }
        tokio::select! {
            _ = sub.changed() => tokio::time::sleep(COALESCE).await,
            _ = tokio::time::sleep(REFRESH) => {}
            _ = sink.closed() => return Ok(()),
        }
        let _ = sub.try_take();
    }
}

/// The message describing an uploaded firmware image.
pub(crate) fn uploaded_firmware(app: &App, id: &str, size: u64, info: &FirmwareInfo) -> pb::UploadedFirmware {
    let model = app.store.model();
    pb::UploadedFirmware {
        upload_id: id.to_owned(),
        size,
        version: info.version.clone(),
        model: info.model.parse().map(crate::convert::model).unwrap_or_default().into(),
        is_stock: info.is_stock,
        model_matches: info.model.is_empty() || info.model.eq_ignore_ascii_case(model.name()),
    }
}
