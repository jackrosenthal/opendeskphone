import { useMutation, useQuery } from "@connectrpc/connect-query";
import { useRef, useState, type ReactNode } from "react";
import { Alert, Badge, Button, Card, ConfirmButton, Dpma, KeyValue, Spinner, Toggle } from "../components/ui";
import { Bank, FirmwareService, TrialBoot, UpdatePhase } from "../gen/opendeskphone/v1/firmware_pb";
import { bytes } from "../lib/format";
import { bankNames, label, modelName, updatePhaseNames } from "../lib/labels";
import { useStatus } from "../lib/status";
import { upload } from "../lib/upload";
import { ErrorNote } from "./common";
import { viaDpma } from "./Settings";

function Bar({ fraction, label }: { fraction: number; label: ReactNode }) {
  return (
    <div>
      <div className="mb-1 flex justify-between text-sm">
        <span>{label}</span>
        <span className="tabular-nums text-slate-500">{Math.round(fraction * 100)}%</span>
      </div>
      <div className="h-2.5 overflow-hidden rounded-full bg-slate-200 dark:bg-slate-700" role="progressbar" aria-valuenow={Math.round(fraction * 100)} aria-valuemin={0} aria-valuemax={100}>
        <div className="h-full rounded-full bg-brand-600 transition-[width] duration-300" style={{ width: `${Math.min(100, fraction * 100)}%` }} />
      </div>
    </div>
  );
}

// Rough share of the whole job per phase, for one overall bar.
const span: Partial<Record<UpdatePhase, [number, number]>> = {
  [UpdatePhase.CHECKING]: [0, 0.03],
  [UpdatePhase.ERASING]: [0.03, 0.15],
  [UpdatePhase.WRITING_KERNEL]: [0.15, 0.2],
  [UpdatePhase.WRITING_ROOTFS]: [0.2, 0.75],
  [UpdatePhase.VERIFYING]: [0.75, 0.98],
  [UpdatePhase.COMMITTING]: [0.98, 1],
};

function ProvisionedFirmware({ onError }: { onError: (e: unknown) => void }) {
  const policy = useQuery(FirmwareService.method.getProvisionedFirmwarePolicy, {});
  const set = useMutation(FirmwareService.method.setProvisionedFirmwarePolicy, { onError, onSuccess: () => (onError(undefined), policy.refetch()) });
  const ignore = policy.data?.ignore;
  const dpma = viaDpma(ignore);
  return (
    <Dpma on={dpma} className="border-t border-slate-100 pt-4 dark:border-slate-800">
      <Toggle label="Ignore DPMA firmware updates" checked={ignore?.value ?? false} disabled={!ignore || ignore.locked || set.isPending} onChange={(v) => set.mutate({ ignore: v })} />
    </Dpma>
  );
}

const version = (v: string) => v.replace(/_/g, ".");

/// The running firmware and the upload that replaces it, on the System page.
export function FirmwareCard() {
  const { status } = useStatus();
  const fw = status?.firmware;
  const file = useRef<HTMLInputElement>(null);
  const [sending, setSending] = useState<number>();
  const [error, setError] = useState<unknown>();
  // The upload waiting for confirmation is not part of the phone's state
  // (nothing changes there until it is installed), so it is asked for.
  const pending = useQuery(FirmwareService.method.getFirmware, {});
  const opts = {
    onError: setError,
    onSuccess: () => {
      setError(undefined);
      pending.refetch();
    },
  };
  const install = useMutation(FirmwareService.method.installFirmware, opts);
  const discard = useMutation(FirmwareService.method.discardUpload, opts);
  const confirm = useMutation(FirmwareService.method.confirmTrial, opts);
  const rollback = useMutation(FirmwareService.method.rollback, opts);

  if (!fw) {
    return (
      <Card title="Firmware">
        <Spinner />
      </Card>
    );
  }
  const up = pending.data?.status?.uploaded;
  const working = fw.phase !== UpdatePhase.IDLE && fw.phase !== UpdatePhase.COMPLETE && fw.phase !== UpdatePhase.FAILED;
  // A D80 receives an upload without knowing its length: the browser's
  // share of the upload is the progress then.
  const receiving = fw.phase === UpdatePhase.RECEIVING && sending !== undefined;
  const [from, to] = span[fw.phase] ?? [0, 0];
  const overall = from + ((to - from) * fw.percent) / 100;

  async function send(f: File) {
    setError(undefined);
    setSending(0);
    try {
      await upload("/upload/firmware", f, setSending);
    } catch (e) {
      setError(e);
    } finally {
      await pending.refetch();
      setSending(undefined);
    }
  }

  return (
    <Card title="Firmware" actions={working && <Badge tone="blue">Updating</Badge>}>
      <div className="space-y-4">
        <ErrorNote error={error} />
        {fw.trial === TrialBoot.PENDING && (
          <Alert tone="blue" title={`Firmware trial: ${fw.trialBootsLeft} ${fw.trialBootsLeft === 1 ? "boot" : "boots"} left.`}>
            <div className="mt-2 flex gap-2">
              <Button size="sm" onClick={() => confirm.mutate({})}>
                Keep
              </Button>
              <ConfirmButton size="sm" variant="secondary" label="Roll Back" title="Roll back to the previous firmware?" confirmLabel="Roll Back" onConfirm={() => rollback.mutateAsync({})} />
            </div>
          </Alert>
        )}
        {fw.trial === TrialBoot.ROLLED_BACK && <Alert tone="red" title="The last firmware update was rolled back.">{fw.rollbackReason}</Alert>}
        <KeyValue
          rows={[
            ["Version", `Open Desk Phone ${fw.runningVersion}`],
            ["Active bank", fw.runningBank !== Bank.UNSPECIFIED ? label(bankNames, fw.runningBank, Bank[fw.runningBank]) : ""],
          ]}
        />
        {working && !receiving ? (
          <Bar fraction={overall} label={<>{label(updatePhaseNames, fw.phase, UpdatePhase[fw.phase])}{fw.incomingVersion && ` · ${version(fw.incomingVersion)}`}</>} />
        ) : fw.phase === UpdatePhase.COMPLETE ? (
          <Alert tone="green" title={`Firmware ${version(fw.incomingVersion)} installed. Reboot to activate.`} />
        ) : sending !== undefined ? (
          <Bar fraction={sending} label="Uploading" />
        ) : up ? (
          <div className="space-y-4 border-t border-slate-100 pt-4 dark:border-slate-800">
            <KeyValue
              rows={[
                ["Upload", `${up.isStock ? "Digium" : "Open Desk Phone"} ${version(up.version)}`],
                ["Model", modelName(up.model)],
                ["Size", bytes(up.size)],
              ]}
            />
            {!up.modelMatches && <Alert tone="red" title={modelName(up.model) ? `This firmware is for the ${modelName(up.model)}.` : "This firmware is for another model."} />}
            <div className="flex gap-2">
              {up.isStock ? (
                <ConfirmButton label="Install" title="Install stock Digium firmware?" confirmLabel="Install" onConfirm={() => install.mutateAsync({ uploadId: up.uploadId, allowStock: true })} />
              ) : (
                <Button variant="primary" disabled={!up.modelMatches} busy={install.isPending} onClick={() => install.mutate({ uploadId: up.uploadId, allowStock: false })}>
                  Install
                </Button>
              )}
              <Button onClick={() => discard.mutate({ uploadId: up.uploadId })}>Discard</Button>
            </div>
          </div>
        ) : (
          <>
            {fw.phase === UpdatePhase.FAILED && <Alert tone="red" title="Firmware update failed.">{fw.error}</Alert>}
            <input ref={file} type="file" accept=".eff" className="hidden" onChange={(e) => e.target.files?.[0] && send(e.target.files[0]).then(() => (e.target.value = ""))} />
            <Button variant="primary" onClick={() => file.current?.click()}>
              Upload Firmware
            </Button>
          </>
        )}
        <ProvisionedFirmware onError={setError} />
      </div>
    </Card>
  );
}
