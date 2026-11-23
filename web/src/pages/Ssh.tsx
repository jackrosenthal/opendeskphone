import { useMutation, useQuery } from "@connectrpc/connect-query";
import { useRef, useState, type FormEvent } from "react";
import { Badge, Button, Card, ConfirmButton, Dpma, Empty, Modal, Select, Toggle, td, th } from "../components/ui";
import { ConfigService } from "../gen/opendeskphone/v1/config_pb";
import { SshService, type SshStatus } from "../gen/opendeskphone/v1/ssh_pb";
import { SystemService } from "../gen/opendeskphone/v1/system_pb";
import { ErrorNote } from "./common";
import { viaDpma } from "./Settings";

/// The switch for the unauthenticated `/metrics` endpoint.
export function MetricsSwitch() {
  const [error, setError] = useState<unknown>();
  const loaded = useQuery(SystemService.method.getMetricsEnabled, {});
  const set = useMutation(SystemService.method.setMetricsEnabled, { onError: setError, onSuccess: () => (setError(undefined), loaded.refetch()) });
  const setting = loaded.data?.metricsEnabled;
  const dpma = viaDpma(setting);
  return (
    <div className="space-y-2 border-t border-slate-100 pt-4 dark:border-slate-800">
      <ErrorNote error={error ?? loaded.error} />
      <Dpma on={dpma}>
        <Toggle label="Prometheus metrics" checked={setting?.value ?? false} disabled={!setting || setting.locked || set.isPending} onChange={(v) => set.mutate({ metricsEnabled: v })} />
      </Dpma>
    </div>
  );
}

/// The switch for this interface, which asks before it locks the
/// administrator out.
export function WebSwitch() {
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<unknown>();
  const onError = setError;
  const list = useQuery(ConfigService.method.listSettings, {});
  const set = useMutation(ConfigService.method.setSettings, { onError, onSuccess: () => (onError(undefined), setAsking(false), list.refetch()) });
  const setting = list.data?.settings.find((s) => s.id === "web_ui_enabled");
  const on = setting ? setting.value === "1" || setting.value.toLowerCase() === "true" || setting.value.toLowerCase() === "yes" : true;
  const save = (v: boolean) => set.mutate({ changes: [{ id: "web_ui_enabled", value: v ? "1" : "0" }] });
  return (
    <>
      <ErrorNote error={error ?? list.error} />
      <Dpma on={setting?.provisioned}>
        <Toggle label="Enabled" checked={on} disabled={!setting || setting.frozen || set.isPending} onChange={(v) => (v ? save(true) : setAsking(true))} />
      </Dpma>
      {asking && (
        <Modal
          title="Disable the web UI?"
          onClose={() => setAsking(false)}
          footer={
            <>
              <Button onClick={() => setAsking(false)}>Cancel</Button>
              <Button variant="danger" busy={set.isPending} onClick={() => save(false)}>
                Disable
              </Button>
            </>
          }
        >
          {null}
        </Modal>
      )}
    </>
  );
}

/// The EHS header as seen when attaching a USB serial cable (two rows of
/// three; position 3 is the key), with the cable's wire colors and each
/// pin's function from the phone's side.
function EhsPinout() {
  const col = [120, 160, 200];
  const row = [72, 108];
  const at = (pin: number) => ({ x: col[(pin - 1) % 3], y: row[pin > 3 ? 1 : 0] });
  const ink = "fill-slate-700 dark:fill-slate-200";
  const muted = "fill-slate-500 dark:fill-slate-400";
  // Each wire is drawn over a wider gray line, so that white shows on the
  // light theme and black on the dark one.
  const wire = (x: number, y1: number, y2: number, color: string) => (
    <>
      <path d={`M${x} ${y1}V${y2}`} className="stroke-slate-500" strokeWidth={8} strokeLinecap="round" />
      <path d={`M${x} ${y1}V${y2}`} stroke={color} strokeWidth={5} strokeLinecap="round" />
    </>
  );
  const caption = (x: number, y: number, color: string, what: string) => (
    <>
      <text x={x} y={y} className={ink} fontWeight={600}>
        {color}
      </text>
      <text x={x} y={y + 13} className={muted}>
        {what}
      </text>
    </>
  );
  return (
    <svg
      viewBox="14 0 250 186"
      className="mx-auto block h-auto w-full max-w-xs"
      fontSize={11}
      textAnchor="middle"
      role="img"
      aria-label="Top row, left to right: pin 1, ground, black wire; pin 2, TX, white wire; key. Bottom row, left to right: pin 4, 3.3 V, not connected; pin 5, RX, green wire; pin 6, ground."
    >
      <rect x={96} y={50} width={128} height={80} rx={4} className="fill-slate-200 stroke-slate-400 dark:fill-slate-800 dark:stroke-slate-600" />
      {[1, 2, 4, 5, 6].map((pin) => (
        <g key={pin}>
          <rect x={at(pin).x - 11} y={at(pin).y - 11} width={22} height={22} rx={2} className="fill-amber-400" />
          <text x={at(pin).x} y={at(pin).y + 4} fontWeight={700} fill="#0f172a">
            {pin}
          </text>
        </g>
      ))}
      <rect x={at(3).x - 11} y={at(3).y - 11} width={22} height={22} rx={2} fill="none" strokeDasharray="3 3" className="stroke-slate-400 dark:stroke-slate-500" />
      <text x={232} y={at(3).y + 4} textAnchor="start" className={muted}>
        Key
      </text>
      {wire(at(1).x, 57, 34, "#111111")}
      {wire(at(2).x, 57, 34, "#ffffff")}
      {wire(at(5).x, 123, 150, "#16a34a")}
      {caption(at(1).x, 12, "Black", "GND")}
      {caption(at(2).x, 12, "White", "TX")}
      {caption(at(5).x, 166, "Green", "RX")}
      <text x={88} y={at(4).y - 2} textAnchor="end" className={ink}>
        3.3 V
      </text>
      <text x={88} y={at(4).y + 10} textAnchor="end" fontSize={10} className={muted}>
        Not connected
      </text>
      <text x={232} y={at(6).y + 4} textAnchor="start" className={ink}>
        GND
      </text>
    </svg>
  );
}

/// UART A is the EHS headset adapter's unless the serial console is chosen.
export function EhsPortCard() {
  const [error, setError] = useState<unknown>();
  const loaded = useQuery(SystemService.method.getSerialConsole, {});
  const set = useMutation(SystemService.method.setSerialConsole, { onError: setError, onSuccess: () => (setError(undefined), loaded.refetch()) });
  const setting = loaded.data?.serialConsole;
  // The choice as made, before the phone has confirmed it.
  const serial = set.isPending ? !!set.variables?.serialConsole : !!setting?.value;
  const dpma = viaDpma(setting);
  return (
    <Card title="EHS Port">
      <div className="space-y-4">
        <ErrorNote error={error ?? loaded.error} />
        <Dpma on={dpma}>
          <Select aria-label="EHS port" value={serial ? "console" : "ehs"} disabled={!setting || setting.locked || set.isPending} onChange={(e) => set.mutate({ serialConsole: e.target.value === "console" })}>
            <option value="ehs">Headset</option>
            <option value="console">Serial Console (115200 8N1)</option>
          </Select>
        </Dpma>
        {serial && <EhsPinout />}
      </div>
    </Card>
  );
}

export function SshCard() {
  const [status, setStatus] = useState<SshStatus>();
  const [error, setError] = useState<unknown>();
  const [key, setKey] = useState("");
  const file = useRef<HTMLInputElement>(null);
  const loaded = useQuery(SshService.method.getSsh, {});
  const done = (r: { status?: SshStatus }) => {
    setError(undefined);
    setStatus(r.status);
  };
  const opts = { onSuccess: done, onError: setError };
  const enable = useMutation(SshService.method.setSshEnabled, opts);
  const add = useMutation(SshService.method.addSshKey, { onSuccess: (r: { status?: SshStatus }) => (done(r), setKey("")), onError: setError });
  const remove = useMutation(SshService.method.removeSshKey, opts);

  const ssh = status ?? loaded.data?.status;
  const enabled = ssh?.enabled;
  const dpma = viaDpma(enabled);
  const keys = ssh?.keys ?? [];
  // The listener's real state, when it disagrees with the switch.
  const stopped = enabled?.value && ssh?.listening === false;

  function submit(e: FormEvent) {
    e.preventDefault();
    // A private key never leaves the browser (the phone refuses one too).
    if (/PRIVATE KEY|PuTTY-User-Key-File/.test(key)) {
      setKey("");
      setError(new Error("This is a private key. Use the public key (.pub) instead."));
    } else if (key.trim()) add.mutate({ key });
  }

  return (
    <Card title="SSH" className="lg:col-span-2">
      <div className="space-y-4">
        <ErrorNote error={error ?? loaded.error} />
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
          <Dpma on={dpma}>
            <Toggle label="Enabled" checked={enabled?.value ?? false} disabled={!ssh || enabled?.locked || enable.isPending} onChange={(v) => enable.mutate({ enabled: v })} />
          </Dpma>
          {stopped && <Badge tone="red">Not running</Badge>}
        </div>

        <div className="border-t border-slate-100 pt-4 dark:border-slate-800">
          <h3 className="mb-2 text-sm font-semibold">Keys</h3>
          {keys.length === 0 ? (
            <Empty>No keys</Empty>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full">
                <thead>
                  <tr>
                    <th className={th}>Type</th>
                    <th className={th}>Fingerprint</th>
                    <th className={th}>Comment</th>
                    <th className={th} />
                  </tr>
                </thead>
                <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                  {keys.map((k) => (
                    <tr key={k.fingerprint}>
                      <td className={td}>
                        <span className="whitespace-nowrap">
                          {k.type}
                          {k.bits > 0 && <span className="text-slate-500"> {k.bits}</span>}
                        </span>
                      </td>
                      <td className={`${td} break-all font-mono text-xs`}>{k.fingerprint}</td>
                      <td className={`${td} break-all`}>{k.comment}</td>
                      <td className={`${td} text-right`}>
                        {k.provisioned ? (
                          <Dpma on className="inline-block">
                            <Button size="sm" disabled>
                              Remove
                            </Button>
                          </Dpma>
                        ) : (
                          <ConfirmButton size="sm" variant="secondary" label="Remove" title={`Remove ${k.comment || k.type}?`} confirmLabel="Remove" onConfirm={() => remove.mutateAsync({ fingerprint: k.fingerprint })} />
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>

        <form onSubmit={submit} className="space-y-2">
          <textarea
            aria-label="Public key"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            rows={3}
            spellCheck={false}
            placeholder="ssh-ed25519 AAAA… name"
            className="block w-full rounded-lg border border-slate-300 bg-white px-3 py-2 font-mono text-xs text-slate-900 placeholder:text-slate-400 focus:border-brand-500 focus:outline-none focus:ring-2 focus:ring-brand-500/30 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
          />
          <div className="flex gap-2">
            <Button type="submit" variant="primary" busy={add.isPending} disabled={!key.trim()}>
              Add Key
            </Button>
            <input
              ref={file}
              type="file"
              accept=".pub,text/plain"
              className="hidden"
              onChange={(e) => {
                const f = e.target.files?.[0];
                e.target.value = "";
                // A public key file is a few hundred bytes; anything large is not one.
                if (f && f.size <= 16384) f.text().then(setKey);
                else if (f) setError(new Error("File is too large to be a public key."));
              }}
            />
            <Button onClick={() => file.current?.click()}>Choose File</Button>
          </div>
        </form>
      </div>
    </Card>
  );
}
