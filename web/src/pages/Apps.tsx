import { useMutation, useQuery } from "@connectrpc/connect-query";
import { useRef, useState } from "react";
import { Badge, Button, Card, ConfirmButton, Dpma, Empty, PageHeader, Spinner, Toggle } from "../components/ui";
import { AppsService } from "../gen/opendeskphone/v1/apps_pb";
import { useStreamState } from "../lib/stream";
import { client } from "../lib/transport";
import { upload } from "../lib/upload";
import { ErrorNote } from "./common";
import { viaDpma } from "./Settings";

export function Apps() {
  const [msg] = useStreamState((signal) => client(AppsService).watchApps({}, { signal }));
  const [error, setError] = useState<unknown>();
  const [busy, setBusy] = useState(false);
  const file = useRef<HTMLInputElement>(null);
  const opts = { onError: setError, onSuccess: () => setError(undefined) };
  const launch = useMutation(AppsService.method.launchApp, opts);
  const close = useMutation(AppsService.method.closeApp, opts);
  const remove = useMutation(AppsService.method.removeApp, opts);
  const disabled = useQuery(AppsService.method.getDisabledApps, {});
  const enable = useMutation(AppsService.method.setAppEnabled, { onError: setError, onSuccess: () => (setError(undefined), disabled.refetch()) });
  const dpma = viaDpma(disabled.data?.disabledApps);
  const locked = !disabled.data || disabled.data.disabledApps?.locked || enable.isPending;

  async function install(f: File) {
    setBusy(true);
    setError(undefined);
    try {
      const up = await upload("/upload/app", f);
      await client(AppsService).installApp({ uploadId: up.uploadId });
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  }

  const snap = msg?.snapshot;
  return (
    <>
      <PageHeader
        title="Apps"
        actions={
          <>
            <input ref={file} type="file" accept=".wasm,application/wasm" className="hidden" onChange={(e) => e.target.files?.[0] && install(e.target.files[0]).then(() => (e.target.value = ""))} />
            <Button variant="primary" busy={busy} onClick={() => file.current?.click()}>
              Install App
            </Button>
          </>
        }
      />
      <div className="space-y-5">
        <ErrorNote error={error ?? disabled.error} />
        {busy && (
          <p className="text-sm text-slate-500 dark:text-slate-400">
            The phone is compiling the app: a few seconds for a small one, a few minutes for the largest. Calls are not disturbed, and the phone can be used meanwhile.
          </p>
        )}
        <Card>
          {!snap ? (
            <Spinner />
          ) : snap.apps.length === 0 ? (
            <Empty>No apps</Empty>
          ) : (
            <ul className="divide-y divide-slate-100 dark:divide-slate-800">
              {snap.apps.map((a) => (
                <li key={a.name} className="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0 last:pb-0">
                  <div className="min-w-0">
                    <p className="font-medium">
                      {a.displayName} <span className="ml-1 font-mono text-xs font-normal text-slate-400">{a.name}</span>
                    </p>
                    <p className="text-sm text-slate-500 dark:text-slate-400">{a.description}</p>
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    {a.builtin && <Badge>Built-in</Badge>}
                    {a.hidden && <Badge>Hidden</Badge>}
                    {snap.idleApp === a.name && <Badge tone="blue">Idle Screen</Badge>}
                    {a.foreground ? <Badge tone="green">Active</Badge> : a.running && <Badge tone="blue">Running</Badge>}
                    {a.disabled ? null : a.running ? (
                      <Button size="sm" onClick={() => close.mutate({ name: a.name })}>
                        Close
                      </Button>
                    ) : (
                      <Button size="sm" onClick={() => launch.mutate({ name: a.name })}>
                        Open
                      </Button>
                    )}
                    {a.builtin && (
                      <Dpma on={dpma}>
                        <Toggle label="Enabled" checked={!a.disabled} disabled={locked} onChange={(v) => enable.mutate({ name: a.name, enabled: v })} />
                      </Dpma>
                    )}
                    {!a.builtin && <ConfirmButton size="sm" variant="secondary" label="Remove" title={`Remove ${a.displayName}?`} confirmLabel="Remove" onConfirm={() => remove.mutateAsync({ name: a.name })} />}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </Card>
      </div>
    </>
  );
}
