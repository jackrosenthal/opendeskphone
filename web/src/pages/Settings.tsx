import { useMutation, useQuery } from "@connectrpc/connect-query";
import { useMemo, useState, type ReactNode } from "react";
import { Button, Card, Dpma, Input, PageHeader, Select, Toggle } from "../components/ui";
import type { IdleSoftkey } from "../gen/opendeskphone/settings/v1/settings_pb";
import { ConfigService, SettingSource, type IdleSoftkeys as IdleSoftkeysMessage, type Setting } from "../gen/opendeskphone/v1/config_pb";
import type { Theme as ThemeMessage } from "../gen/opendeskphone/v1/config_pb";
import { Skin } from "../gen/opendeskphone/v1/model_pb";
import { useStatus } from "../lib/status";
import { ErrorNote } from "./common";

type Kind = { type: "bool" } | { type: "number"; min: number; max: number } | { type: "text"; placeholder?: string } | { type: "choice"; options: [string, string][] };
/// One stock setting in a form.  `grayOnly`: the setting exists only on
/// the gray-display models.  `wide`: the field spans the card.
/// A field with a `fallback` shows that setting's value while its own
/// setting has none.
export type Item = { id: string; label: string; hint?: string; grayOnly?: boolean; wide?: boolean; fallback?: string } & Kind;

const truthy = (v: string) => v === "1" || v.toLowerCase() === "true" || v.toLowerCase() === "yes";

function Control({ item, setting, value, onChange }: { item: Item; setting?: Setting; value: string; onChange: (v: string) => void }) {
  // As on stock's web UI, only a frozen setting is disabled: a value set
  // here replaces any other provisioned one.
  const frozen = setting?.frozen;
  if (item.type === "bool") {
    return (
      <Dpma on={setting?.provisioned}>
        <Toggle label={item.label} checked={truthy(value)} disabled={frozen} onChange={(v) => onChange(v ? "1" : "0")} />
        {item.hint && <p className="mt-1 pl-14 text-xs text-slate-500 dark:text-slate-400">{item.hint}</p>}
      </Dpma>
    );
  }
  return (
    <Dpma on={setting?.provisioned}>
    <label className="block">
      <span className="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">{item.label}</span>
      {item.type === "choice" ? (
        <Select value={value} disabled={frozen} onChange={(e) => onChange(e.target.value)}>
          {!item.options.some(([v]) => v === value) && <option value={value}>{value}</option>}
          {item.options.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </Select>
      ) : item.type === "number" ? (
        <Input type="number" min={item.min} max={item.max} value={value} disabled={frozen} onChange={(e) => onChange(e.target.value)} />
      ) : (
        <Input value={value} placeholder={item.placeholder} disabled={frozen} onChange={(e) => onChange(e.target.value)} />
      )}
      {item.hint && <span className="mt-1 block text-xs text-slate-500 dark:text-slate-400">{item.hint}</span>}
    </label>
    </Dpma>
  );
}

/// A card of stock settings with its own Save button.  `children` come
/// before the fields, `after` after them, and `actions` before the Save
/// button.
export function SettingsCard({ title, items, children, after, actions, className }: { title: string; items: Item[]; children?: ReactNode; after?: ReactNode; actions?: ReactNode; className?: string }) {
  const list = useQuery(ConfigService.method.listSettings, {});
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [error, setError] = useState<unknown>();
  const skin = useStatus().status?.system?.skin;
  const save = useMutation(ConfigService.method.setSettings, {
    onError: setError,
    onSuccess: () => {
      setError(undefined);
      list.refetch().then(() => setDraft({}));
    },
  });
  const byId = useMemo(() => new Map((list.data?.settings ?? []).map((s) => [s.id, s])), [list.data]);
  const saved = (item: Item) => byId.get(item.id)?.value || (item.fallback && byId.get(item.fallback)?.value) || "";
  const value = (item: Item) => draft[item.id] ?? saved(item);
  const change = (item: Item, v: string) => {
    const next = { ...draft };
    if (v === saved(item)) delete next[item.id];
    else next[item.id] = v;
    setDraft(next);
  };
  const changes = Object.entries(draft).map(([id, v]) => ({ id, value: v }));
  const dirty = changes.length > 0;

  return (
    <Card
      title={title}
      className={className}
      actions={
        <>
          {actions}
          {dirty && (
            <Button size="sm" onClick={() => setDraft({})}>
              Discard
            </Button>
          )}
          <Button size="sm" variant="primary" disabled={!dirty} busy={save.isPending} onClick={() => save.mutate({ changes })}>
            Save
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <ErrorNote error={error ?? list.error} />
        {children}
        <div className="grid gap-4 sm:grid-cols-2">
          {items
            .filter((item) => !item.grayOnly || skin !== Skin.COLOR)
            .map((item) => (
              <div key={item.id} className={item.type === "bool" || item.wide ? "sm:col-span-2" : ""}>
                <Control item={item} setting={byId.get(item.id)} value={value(item)} onChange={(v) => change(item, v)} />
              </div>
            ))}
        </div>
        {after}
      </div>
    </Card>
  );
}

const sound: Item[] = [
  { id: "ringer_volume", label: "Ringer volume", type: "number", min: 0, max: 10 },
  { id: "handset_volume", label: "Handset volume", type: "number", min: 0, max: 10 },
  { id: "headset_volume", label: "Headset volume", type: "number", min: 0, max: 10 },
  { id: "speaker_volume", label: "Speaker volume", type: "number", min: 0, max: 10 },
  // Stock's web UI and phone menu set `active_ringtone`, and
  // `default_ringtone` names the ringtone of the image and is frozen.
  { id: "active_ringtone", label: "Ringtone", type: "text", fallback: "default_ringtone" },
  { id: "ehs", label: "EHS headset", type: "choice", options: [["auto", "Auto"], ["plantronics", "Plantronics"], ["jabra_iq", "Jabra"], ["none", "None"]] },
  { id: "reset_call_volume", label: "Reset call volume after each call", type: "bool" },
  { id: "lock_ringing_volume", label: "Lock ringer volume", type: "bool" },
  { id: "call_waiting_tone", label: "Call waiting tone", type: "bool" },
  { id: "ring_headset_only", label: "Ring headset only", type: "bool" },
  { id: "headset_answer", label: "Answer with headset", type: "bool" },
];

const display: Item[] = [
  { id: "brightness", label: "Brightness", type: "number", min: 0, max: 10 },
  { id: "contrast", label: "Contrast", type: "number", min: 0, max: 10, grayOnly: true },
  { id: "backlight_timeout", label: "Backlight timeout (seconds)", type: "number", min: 0, max: 86400, hint: "0 = always on" },
  { id: "name_format", label: "Name format", type: "choice", options: [["first_last", "First Last"], ["last_first", "Last, First"]] },
  { id: "time_short_format", label: "Time format", type: "text" },
  { id: "date_short_format", label: "Date format", type: "text" },
  { id: "dim_backlight", label: "Dim backlight when idle", type: "bool" },
  { id: "backlight_dim_level", label: "Dim level", type: "number", min: 0, max: 10, hint: "0 = off" },
  { id: "small_clock", label: "Small clock", type: "bool" },
];

/// True when the two softkeys are the same one.
function sameSoftkey(a: IdleSoftkey | undefined, b: IdleSoftkey): boolean {
  if (a?.action.case === "function" && b.action.case === "function") return a.action.value === b.action.value;
  return a?.action.case === "app" && b.action.case === "app" && a.action.value.name === b.action.value.name;
}

/// The softkeys of the phone's idle screen, in order.  Each change is
/// saved as it is made.
function IdleSoftkeys() {
  const [error, setError] = useState<unknown>();
  const [saved, setSaved] = useState<IdleSoftkeysMessage>();
  const loaded = useQuery(ConfigService.method.getIdleSoftkeys, {});
  const set = useMutation(ConfigService.method.setIdleSoftkeys, {
    onError: setError,
    onSuccess: (r) => {
      setError(undefined);
      setSaved(r.idleSoftkeys);
    },
  });
  const keys = saved ?? loaded.data?.idleSoftkeys;
  const softkeys = keys?.softkeys ?? [];
  const label = (key: IdleSoftkey) => keys?.choices.find((c) => sameSoftkey(c.softkey, key))?.label ?? (key.action.case === "app" ? key.action.value.name : "");
  const unused = (keys?.choices ?? []).filter((c) => c.softkey && !softkeys.some((k) => sameSoftkey(c.softkey, k)));
  const dpma = viaDpma(keys);
  const fixed = !keys || keys.locked || set.isPending;
  const save = (softkeys: IdleSoftkey[]) => set.mutate({ softkeys });
  const move = (i: number, by: number) => {
    const next = [...softkeys];
    [next[i], next[i + by]] = [next[i + by], next[i]];
    save(next);
  };
  return (
    <Card
      title="Idle Softkeys"
      actions={
        keys?.source === SettingSource.LOCAL &&
        !keys.locked && (
          <Button size="sm" disabled={set.isPending} onClick={() => save([])}>
            Reset
          </Button>
        )
      }
    >
      <Dpma on={dpma} className="space-y-3">
        <ErrorNote error={error ?? loaded.error} />
        <ol className="divide-y divide-slate-100 dark:divide-slate-800">
          {softkeys.map((key, i) => (
            <li key={i} className="flex items-center gap-1 py-1.5 text-sm">
              <span className="w-6 text-slate-400">{i + 1}</span>
              <span className="flex-1">{label(key)}</span>
              <Button size="sm" variant="ghost" aria-label="Move up" disabled={fixed || i === 0} onClick={() => move(i, -1)}>
                ↑
              </Button>
              <Button size="sm" variant="ghost" aria-label="Move down" disabled={fixed || i === softkeys.length - 1} onClick={() => move(i, 1)}>
                ↓
              </Button>
              <Button size="sm" variant="ghost" aria-label="Remove" disabled={fixed || softkeys.length === 1} onClick={() => save(softkeys.filter((_, j) => j !== i))}>
                ✕
              </Button>
            </li>
          ))}
        </ol>
        <Select
          aria-label="Add softkey"
          value=""
          disabled={fixed}
          onChange={(e) => {
            const added = e.target.value === "" ? undefined : unused[Number(e.target.value)]?.softkey;
            if (added) save([...softkeys, added]);
          }}
        >
          <option value="">Add softkey…</option>
          {unused.map((c, i) => (
            <option key={i} value={i}>
              {c.label}
            </option>
          ))}
        </Select>
      </Dpma>
    </Card>
  );
}

/// The theme of a color model: its wallpaper and the colors of every
/// screen of the phone.  A change is saved as it is made.
function ThemeSelect() {
  const [error, setError] = useState<unknown>();
  const [saved, setSaved] = useState<ThemeMessage>();
  const skin = useStatus().status?.system?.skin;
  const loaded = useQuery(ConfigService.method.getTheme, {});
  const set = useMutation(ConfigService.method.setTheme, {
    onError: setError,
    onSuccess: (r) => {
      setError(undefined);
      setSaved(r.theme);
    },
  });
  if (skin !== Skin.COLOR) return null;
  const theme = saved ?? loaded.data?.theme;
  const choices = theme?.choices ?? [];
  const families = [...new Set(choices.map((c) => c.family))];
  const dpma = viaDpma(theme);
  return (
    <Dpma on={dpma}>
      <ErrorNote error={error ?? loaded.error} />
      <label className="block">
        <span className="mb-1 block text-sm font-medium text-slate-700 dark:text-slate-300">Theme</span>
        <Select
          value={theme?.picture ? "" : (theme?.id ?? "")}
          disabled={!theme || theme.locked || set.isPending}
          onChange={(e) => e.target.value && set.mutate({ id: e.target.value })}
        >
          {theme?.picture && <option value="">Provisioned picture</option>}
          {families.map((family) => (
            <optgroup key={family} label={family}>
              {choices
                .filter((c) => c.family === family)
                .map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
            </optgroup>
          ))}
        </Select>
      </label>
    </Dpma>
  );
}

/// A control's tooltip says when one of Open Desk Phone's own settings has
/// the provisioning server's value; the control itself is disabled only
/// when the setting is `locked`.
export function viaDpma(s: { locked: boolean; source: SettingSource } | undefined): boolean {
  return !!s && (s.locked || s.source === SettingSource.PROVISIONED);
}

/// What the person at the phone hears and sees.
export function Preferences() {
  return (
    <>
      <PageHeader title="Preferences" />
      <div className="grid items-start gap-5 lg:grid-cols-2">
        <SettingsCard title="Sound" items={sound} />
        <div className="space-y-5">
          <SettingsCard title="Display" items={display} after={<ThemeSelect />} />
          <IdleSoftkeys />
        </div>
      </div>
    </>
  );
}
