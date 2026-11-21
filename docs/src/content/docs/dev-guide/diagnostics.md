---
title: "Logs and diagnostics"
---

## Log locations

| Source | Destination | Retention |
| --- | --- | --- |
| Main role | A ring buffer in the main role's memory: the last 2,000 entries or 256 KiB | Until the main role restarts |
| Init role | The kernel log | Until the phone restarts |
| Kernel | The kernel log.  The last 8 KiB of console output, and the last 8 KiB of the log at a panic, are also kept in memory that survives a reset. | Across a reset, but not a power loss |

## Main role log

```sh
dphone log            # the last 200 entries
dphone log -n 50 -f   # the last 50, then follow
```

The **Logs** page of the web interface shows the same ring buffer live, with filters for level and text.

On a phone, the main role logs at the info level and above; with `--verbose`, the simulator logs at the debug level.

To keep the log across a restart of the main role, save the log on the workstation:

```sh
ssh root@192.0.2.20 dphone log -f | tee phone.log
```

To send every entry to a syslog server, use the **Remote syslog** switch, the server, the port, and the minimum level in the **Logging** card of the **System** page.

## Kernel log

```sh
dmesg
```

The init role writes to the kernel log, so `dmesg` shows the kernel's messages and the init role's decisions in one sequence: the modules loaded, each start and failure of the main role, a start in safe mode, the trial boot, and a rollback to the previous bank.  When the main role does not stay up, check `dmesg` first: the main role's ring buffer disappears with the failed process.

## Crash records

The kernel keeps two records in memory that survives a reset:

- **The panic log.**  At the next start after a kernel panic, the main role moves the last 8 KiB of the kernel log to `/nvdata/opendeskphone/crash/`, replacing the records of any earlier panic, and logs a message.  `dphone crashlog` prints the saved records.
- **The console output.**  `/sys/fs/pstore/console-ramoops-0` holds the last 8 KiB of the kernel's console output before a restart without a power loss, e.g., a watchdog reset, until the next restart.  After an unexplained restart, the file shows what the kernel printed last.

```sh
dphone crashlog
cat /sys/fs/pstore/console-ramoops-0
```

## Runtime state

### Health

```sh
dphone health
```

The command lists the conditions for the main role's health report to the init role, and whether each condition is met.  The same report confirms a [trial boot](firmware-updates.mdx#trial-boot), so `dphone health` shows why a trial is unconfirmed.  A subsystem whose loop has stopped shows as degraded here, in the web interface, and in the log.

### State store

```sh
dphone state
dphone state audio
```

Every subsystem publishes state to one typed store, which the user interface, the web interface, and the commands read.  When the screen or the web interface disagrees with what the phone is doing, the dump shows whether the state or the display is wrong.

## Web interface

| Page | Description |
| --- | --- |
| **Screen** | A live mirror of the phone's display, with clickable phone keys.  The D80 has no keys: a click on its mirrored screen is a tap, and a drag is a finger's drag. |
| **Shell** | A root shell in the browser, with the shell and commands of an SSH session.  Works without an SSH key and with the SSH setting off. |

## Metrics

`GET /metrics` returns metrics in the Prometheus text format, without a login:

```sh
curl http://192.0.2.20/metrics
```

The endpoint is on by default.  To turn the endpoint off, use the **Prometheus metrics** switch in the **Logging** card of the **System** page, or set [`opendeskphone_metrics_enabled=no`](../open-dpma/opendeskphone-options.md#opendeskphone_metrics_enabled) on the PBX.
