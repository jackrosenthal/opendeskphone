import { useQuery } from "@connectrpc/connect-query";
import { useCallback, useRef, useState, type CSSProperties, type PointerEvent, type RefObject } from "react";
import { Alert, Card, PageHeader, Spinner } from "../components/ui";
import { Model } from "../gen/opendeskphone/v1/model_pb";
import { KeyAction, Key_Kind as K, LampBlink, LampColor, ScreenId, ScreenService, TouchAction, type Lamp } from "../gen/opendeskphone/v1/screen_pb";
import { modelName } from "../lib/labels";
import { PhoneIcon } from "../lib/phoneIcons";
import { archedRect, key, panels, sameKey, type KeyId, type Panel, type PanelKey } from "../lib/phones";
import { useStream, type StreamStatus } from "../lib/stream";
import { client, errorMessage } from "../lib/transport";

const lampFill: Partial<Record<LampColor, string>> = { [LampColor.RED]: "#ef4444", [LampColor.GREEN]: "#22c55e", [LampColor.AMBER]: "#f59e0b" };
const blinkClass: Partial<Record<LampBlink, string>> = { [LampBlink.SLOW]: "lamp-slow", [LampBlink.FAST]: "lamp-fast", [LampBlink.WINK]: "lamp-wink" };

const keyNames: Partial<Record<K, string>> = {
  [K.STAR]: "*",
  [K.POUND]: "#",
  [K.BLF_PAGE_LEFT]: "Previous page",
  [K.BLF_PAGE_RIGHT]: "Next page",
  [K.HEADSET]: "Headset",
  [K.SPEAKER]: "Speaker",
  [K.MUTE]: "Mute",
  [K.VOLUME_DOWN]: "Volume down",
  [K.VOLUME_UP]: "Volume up",
  [K.SELECT]: "Select",
  [K.CANCEL]: "Cancel",
  [K.UP]: "Up",
  [K.DOWN]: "Down",
  [K.LEFT]: "Left",
  [K.RIGHT]: "Right",
  [K.MESSAGES]: "Messages",
  [K.TRANSFER]: "Transfer",
  [K.REDIAL]: "Redial",
  [K.HOLD]: "Hold",
  [K.CONFERENCE]: "Conference",
  [K.INFO]: "Info",
  [K.APPS]: "Apps",
  [K.CONTACTS]: "Contacts",
  [K.MENU]: "Menu",
  [K.STATUS]: "Status",
};

/// What a key is called, for keys whose print is an icon or nothing.
function keyName(k: KeyId): string {
  switch (k.kind) {
    case K.DIGIT:
      return String(k.index);
    case K.SOFT:
      return `Softkey ${k.index}`;
    case K.LINE:
      return `Line key ${k.index}`;
    case K.BLF:
      return `Rapid dial key ${k.index}`;
    case K.EXPANSION:
      return `Expansion key ${k.module}.${k.index}`;
    case K.RAW:
      return `Key ${k.index}`;
    default:
      return keyNames[k.kind] ?? K[k.kind];
  }
}

/// A React key for a key.
const keyId = (k: KeyId) => `${k.kind}.${k.index}.${k.module}`;

type Meta = { lamps: Lamp[]; backlight: boolean; offHook: boolean };

/// One display's picture, kept on a canvas: the whole picture when one
/// comes, and the parts that changed (`patches`) drawn over it.  Returns
/// the canvas to show it on.
function useMirror(screen: ScreenId, enabled: boolean, onMeta?: (m: Meta) => void): [RefObject<HTMLCanvasElement | null>, StreamStatus] {
  const canvas = useRef<HTMLCanvasElement>(null);
  // The pictures of a message are decoded together, and drawn after those
  // of the messages before it.
  const drawing = useRef(Promise.resolve());
  // Counts the watches.  Each begins with the whole picture: a picture
  // that cannot be drawn leaves the canvas behind the screen, and a new
  // watch, a second later, puts it right.
  const [watch, setWatch] = useState(0);
  const again = () => void setTimeout(() => setWatch((n) => n + 1), 1000);
  const status = useStream(
    (signal) => (enabled ? client(ScreenService).watchScreen({ screen, maxFps: 8, patches: true }, { signal }) : (async function* () {})()),
    (msg) => {
      onMeta?.({ lamps: msg.lamps, backlight: msg.backlightOn, offHook: msg.offHook });
      const whole = msg.png.length > 0;
      const parts = whole ? [{ x: 0, y: 0, png: msg.png }] : msg.patches;
      if (parts.length === 0) return;
      drawing.current = drawing.current
        .then(async () => {
          const pictures = await Promise.all(parts.map((p) => createImageBitmap(new Blob([p.png as BlobPart], { type: "image/png" }))));
          const c = canvas.current;
          const ctx = c?.getContext("2d");
          if (c && ctx) {
            if (whole && (c.width !== msg.width || c.height !== msg.height)) {
              c.width = msg.width;
              c.height = msg.height;
            }
            parts.forEach((p, i) => ctx.drawImage(pictures[i], p.x, p.y));
          }
          pictures.forEach((p) => p.close());
        })
        .catch(again);
    },
    [screen, enabled, watch],
  );
  return [canvas, status];
}

type Touch = { action: TouchAction; x: number; y: number };

/// Sends the touches of the mirrored screen one at a time, in order: the
/// events of a finger must not overtake one another on the way.  Of the
/// moves that wait to be sent, only the latest is (the phone follows the
/// finger by where it is, so a move in between adds nothing).
function useTouches(onResult: (error: string) => void): (t: Touch) => void {
  const waiting = useRef<Touch[]>([]);
  const sending = useRef(false);
  return useCallback(
    (t: Touch) => {
      const q = waiting.current;
      if (t.action === TouchAction.MOVE && q.length > 0 && q[q.length - 1].action === TouchAction.MOVE) q[q.length - 1] = t;
      else q.push(t);
      if (sending.current) return;
      sending.current = true;
      void (async () => {
        for (let next = q.shift(); next; next = q.shift()) {
          try {
            await client(ScreenService).touch(next);
            onResult("");
          } catch (e) {
            onResult(errorMessage(e));
          }
        }
        sending.current = false;
      })();
    },
    [onResult],
  );
}

function Legend({ k }: { k: PanelKey }) {
  const ink = k.ink ?? "#f2f3f4";
  const l = k.legend;
  if (!l) return null;
  if ("icon" in l) {
    const s = k.size ?? 16;
    return (
      <g transform={`translate(${k.cx - s / 2} ${k.cy - s / 2}) scale(${s / 24})`}>
        <PhoneIcon name={l.icon} color={ink} />
      </g>
    );
  }
  if ("text" in l) {
    return (
      <text x={k.cx} y={k.cy} fill={ink} fontSize={k.size ?? 10} fontWeight={600} textAnchor="middle" dominantBaseline="central">
        {l.text}
      </text>
    );
  }
  return (
    <>
      {l.letters && (
        <text x={k.cx} y={k.cy - k.h * 0.27} fill={ink} fontSize={k.h * 0.23} fontWeight={700} textAnchor="middle" dominantBaseline="central">
          {l.letters}
        </text>
      )}
      <text x={k.cx} y={k.cy + (l.letters !== undefined ? k.h * 0.16 : 0)} fill={ink} fontSize={k.h * 0.52} fontWeight={700} textAnchor="middle" dominantBaseline="central">
        {l.digit}
      </text>
    </>
  );
}

type Size = { width: number; height: number };

/// The phone drawn as it is, its screens mirrored, and its keys and its
/// handset working.  With `touch` (the main screen's size), the main
/// screen is a touchscreen: a press and a drag on the picture touch it.
function Phone({ panel, model, keys, sideSize, touch }: { panel: Panel; model: Model; keys: KeyId[]; sideSize?: Size; touch?: Size }) {
  const [meta, setMeta] = useState<Meta>({ lamps: [], backlight: true, offHook: false });
  const [error, setError] = useState("");
  const held = useRef<KeyId>(undefined);
  const [mainCanvas, mainStatus] = useMirror(ScreenId.MAIN, true, setMeta);
  const side = sideSize ? panel.side : undefined;
  const [sideCanvas] = useMirror(ScreenId.SIDE, !!side);
  const sendTouch = useTouches(setError);
  // The pointer that is touching the screen, and the place last sent.
  const finger = useRef<{ id: number; x: number; y: number }>(undefined);

  const press = (key: KeyId, action: KeyAction) =>
    client(ScreenService)
      .pressKey({ key, action })
      .then(() => setError(""))
      .catch((e) => setError(errorMessage(e)));
  const toggleHook = () =>
    client(ScreenService)
      .setHook({ offHook: !meta.offHook })
      .then(() => setError(""))
      .catch((e) => setError(errorMessage(e)));

  const lamp = (k: KeyId) => meta.lamps.find((l) => l.key && sameKey(l.key, k) && l.color !== LampColor.OFF);
  const [vx, vy, vw, vh] = panel.viewBox;
  const d = panel.display;
  const hs = panel.handset;
  const grille = panel.grille;
  const mwi = lamp(key(K.MESSAGES));
  /// Where a box of the drawing is on the page: the drawing keeps its
  /// proportions, so a box is a part of its width and its height.
  const place = (b: { x: number; y: number; w: number; h: number }): CSSProperties => ({
    left: `${(100 * (b.x - vx)) / vw}%`,
    top: `${(100 * (b.y - vy)) / vh}%`,
    width: `${(100 * b.w) / vw}%`,
    height: `${(100 * b.h) / vh}%`,
  });

  /// The screen's pixel under the pointer, from the picture's place and
  /// size on the page (whatever it is scaled to), kept on the screen.
  const pixel = (e: PointerEvent<Element>, size: Size) => {
    const r = e.currentTarget.getBoundingClientRect();
    const at = (v: number, start: number, len: number, n: number) => Math.min(n - 1, Math.max(0, Math.floor(((v - start) / len) * n)));
    return { x: at(e.clientX, r.left, r.width, size.width), y: at(e.clientY, r.top, r.height, size.height) };
  };
  const touchHandlers = (size: Size) => ({
    onPointerDown: (e: PointerEvent<Element>) => {
      if (finger.current || (e.pointerType === "mouse" && e.button !== 0)) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      const p = pixel(e, size);
      finger.current = { id: e.pointerId, ...p };
      sendTouch({ action: TouchAction.DOWN, ...p });
    },
    onPointerMove: (e: PointerEvent<Element>) => {
      const f = finger.current;
      if (f?.id !== e.pointerId) return;
      const p = pixel(e, size);
      if (p.x === f.x && p.y === f.y) return;
      finger.current = { id: f.id, ...p };
      sendTouch({ action: TouchAction.MOVE, ...p });
    },
    onPointerUp: (e: PointerEvent<Element>) => {
      if (finger.current?.id !== e.pointerId) return;
      finger.current = undefined;
      sendTouch({ action: TouchAction.UP, ...pixel(e, size) });
    },
    onPointerCancel: (e: PointerEvent<Element>) => {
      if (finger.current?.id !== e.pointerId) return;
      finger.current = undefined;
      sendTouch({ action: TouchAction.CANCEL, ...pixel(e, size) });
    },
  });

  return (
    <>
      {error && (
        <div className="mb-4">
          <Alert tone="red">{error}</Alert>
        </div>
      )}
      {/* The drawing, and over it the displays' pictures, on canvases. */}
      <div className="relative mx-auto w-full select-none" style={{ maxWidth: panel.fitHeight ? `min(${vw * panel.unit}px, ${(78 * vw) / vh}vh)` : vw * panel.unit }}>
        <svg viewBox={`${vx} ${vy} ${vw} ${vh}`} className="block h-auto w-full" role="group" aria-label={`${modelName(model)} front panel`}>
          <defs>
            <linearGradient id="gloss" x1="0" y1="0" x2="1" y2="1">
              <stop offset="0" stopColor="#fff" stopOpacity="0.16" />
              <stop offset="0.45" stopColor="#fff" stopOpacity="0" />
            </linearGradient>
            <linearGradient id="handset" x1="0" y1="0" x2="1" y2="0">
              <stop offset="0" stopColor="#fff" stopOpacity="0.02" />
              <stop offset="0.4" stopColor="#fff" stopOpacity="0.14" />
              <stop offset="1" stopColor="#000" stopOpacity="0.25" />
            </linearGradient>
            <filter id="lift" x="-30%" y="-10%" width="160%" height="120%">
              <feDropShadow dx="10" dy="12" stdDeviation="8" floodOpacity="0.45" />
            </filter>
          </defs>

          {panel.shapes.map((s, i) => (
            <g key={i}>
              <path d={s.d} fill={s.fill} stroke={s.stroke} opacity={s.opacity} />
              {s.gloss && <path d={s.d} fill="url(#gloss)" />}
            </g>
          ))}

          {/* Speaker grille in the cradle: vertical slots within an oval. */}
          {grille &&
            Array.from({ length: 11 }, (_, i) => {
              const g = grille;
              const x = g.cx + ((i - 5) * g.rx) / 5.6;
              const half = g.ry * Math.sqrt(Math.max(0, 1 - ((x - g.cx) / g.rx) ** 2)) * 0.95;
              const w = g.rx / 9;
              // Each column is a long slot broken by a short bridge, offset
              // from its neighbor, with a round hole at one end.
              const gap = half * (i % 2 ? 0.25 : -0.2);
              return (
                <g key={i} stroke="#08090a" strokeWidth={w} strokeLinecap="round">
                  <path d={`M${x} ${g.cy - half + w}V${g.cy + gap - w * 1.4}`} />
                  <path d={`M${x} ${g.cy + gap + w * 1.4}V${g.cy + half - w}`} />
                  <circle cx={x} cy={g.cy + gap} r={w * 0.2} />
                </g>
              );
            })}

          {/* Displays, dark until their pictures come (on the canvases). */}
          <rect x={d.x} y={d.y} width={d.w} height={d.h} fill="#050506" />
          {panel.side && <rect x={panel.side.x} y={panel.side.y} width={panel.side.w} height={panel.side.h} fill="#050506" />}

          {/* Printed lines from keys to the display. */}
          {panel.lines.map(([x1, y1, x2, y2], i) => (
            <path key={i} d={`M${x1} ${y1}L${x2} ${y2}`} stroke={panel.lineColor} strokeWidth={0.9} />
          ))}

          {/* Message waiting lamp. */}
          {panel.mwi && (
            <>
              <path d={panel.mwi.d} fill={panel.mwi.off}>
                <title>Message waiting</title>
              </path>
              {mwi && <path d={panel.mwi.d} fill={lampFill[mwi.color]} className={blinkClass[mwi.blink]} pointerEvents="none" />}
              <path d={panel.mwi.d} fill="none" stroke="#00000040" strokeWidth={0.7} pointerEvents="none" />
            </>
          )}

          {/* Keys. */}
          {panel.keys
            .filter((k) => keys.some((x) => sameKey(x, k.key)))
            .map((k) => {
              const l = k.lamp ? lamp(k.key) : undefined;
              const box = { x: k.cx - k.w / 2, y: k.cy - k.h / 2, width: k.w, height: k.h, rx: k.r };
              const release = () => {
                if (held.current && sameKey(held.current, k.key)) {
                  held.current = undefined;
                  press(k.key, KeyAction.UP);
                }
              };
              return (
                <g
                  key={keyId(k.key)}
                  role="button"
                  aria-label={keyName(k.key)}
                  className="cursor-pointer outline-none [&:active>.key-face]:brightness-150"
                  onPointerDown={(e) => {
                    e.currentTarget.setPointerCapture(e.pointerId);
                    held.current = k.key;
                    press(k.key, KeyAction.DOWN);
                  }}
                  onPointerUp={release}
                  onPointerCancel={release}
                >
                  <title>{keyName(k.key)}</title>
                  {k.arch ? (
                    <path className="key-face" d={archedRect(box.x, box.y, box.width, box.height, k.r, k.arch)} fill={k.fill} stroke="#00000066" strokeWidth={0.8} />
                  ) : (
                    <rect className="key-face" {...box} fill={k.bare ? "transparent" : k.fill} stroke={k.bare ? "none" : "#00000066"} strokeWidth={0.8} />
                  )}
                  {l &&
                    (k.arch ? (
                      <path d={archedRect(box.x, box.y, box.width, box.height, k.r, k.arch)} fill={lampFill[l.color]} className={blinkClass[l.blink]} />
                    ) : (
                      <rect {...box} fill={lampFill[l.color]} className={blinkClass[l.blink]} />
                    ))}
                  <Legend k={k} />
                </g>
              );
            })}

          {/* Handset, in its cradle; click to lift or replace it. */}
          <g
            role="button"
            aria-label={meta.offHook ? "Hang up handset" : "Pick up handset"}
            aria-pressed={meta.offHook}
            className="cursor-pointer outline-none"
            onClick={toggleHook}
            transform={meta.offHook ? `translate(${-(hs.w + 14)} ${-hs.h * 0.02})` : undefined}
            filter={meta.offHook ? "url(#lift)" : undefined}
          >
            <title>{meta.offHook ? "Hang up handset" : "Pick up handset"}</title>
            {/* Coiled cord from the mouthpiece end. */}
            <path
              d={`M${hs.x + hs.w * 0.5} ${hs.y + hs.h - 6}c0 ${hs.w * 0.3} ${-hs.w * 0.12} ${hs.w * 0.36} ${-hs.w * 0.3} ${hs.w * 0.36}`}
              fill="none"
              stroke="#141516"
              strokeWidth={hs.w * 0.085}
              strokeLinecap="round"
              strokeDasharray={`${hs.w * 0.022} ${hs.w * 0.016}`}
            />
            <rect x={hs.x} y={hs.y} width={hs.w} height={hs.h} rx={hs.w * 0.36} fill={hs.fill} stroke="#00000055" />
            <rect x={hs.x} y={hs.y} width={hs.w} height={hs.h} rx={hs.w * 0.36} fill="url(#handset)" />
            {/* The bevel along the edges; the handset is even along its length. */}
            <rect x={hs.x + hs.w * 0.08} y={hs.y + hs.w * 0.08} width={hs.w * 0.84} height={hs.h - hs.w * 0.16} rx={hs.w * 0.3} fill="none" stroke="#ffffff" strokeOpacity={0.07} strokeWidth={1.2} />
            {hs.print && (
              <text x={hs.x + hs.w * 0.5} y={hs.y + hs.h * 0.86} fill="#aeb4bc" fontSize={hs.w * 0.1} fontWeight={600} textAnchor="middle">
                {hs.print}
              </text>
            )}
          </g>
        </svg>
        {/* On a touchscreen, the picture itself is touched. */}
        <canvas
          ref={mainCanvas}
          className={`absolute ${touch ? "cursor-pointer" : "pointer-events-none"}`}
          style={{ ...place(d), opacity: meta.backlight ? 1 : 0.6, imageRendering: panel.smooth ? "auto" : "pixelated", touchAction: touch ? "none" : undefined }}
          {...(touch ? { role: "application", "aria-label": "Touchscreen", ...touchHandlers(touch) } : {})}
        />
        {side && <canvas ref={sideCanvas} className="pointer-events-none absolute" style={{ ...place(side), imageRendering: "pixelated" }} />}
        {mainStatus === "reconnecting" && (
          <svg viewBox={`0 0 ${d.w} ${d.h}`} className="pointer-events-none absolute" style={place(d)} aria-hidden>
            <text x={d.w / 2} y={d.h / 2} fill="#fbbf24" fontSize={Math.min(d.h / 9, d.w / 15)} textAnchor="middle" dominantBaseline="central">
              Connection lost
            </text>
          </svg>
        )}
      </div>
    </>
  );
}

export function Screen() {
  const layout = useQuery(ScreenService.method.getScreenLayout, {});
  const [error, setError] = useState("");

  if (layout.error) return <Alert tone="red">{errorMessage(layout.error)}</Alert>;
  if (!layout.data) return <Spinner />;
  const { screens, model, mirrorAvailable, touch } = layout.data;
  const keys: KeyId[] = layout.data.keys;
  const main = screens.find((s) => s.id === ScreenId.MAIN);
  const side = screens.find((s) => s.id === ScreenId.SIDE);
  const panel = panels[model];
  // Keys the phone reports that the drawing has no place for: listed
  // plainly rather than given an invented position.
  const unplaced = panel ? keys.filter((k) => !panel.keys.some((p) => sameKey(p.key, k))) : keys;
  const tap = (k: KeyId) =>
    client(ScreenService)
      .pressKey({ key: k, action: KeyAction.TAP })
      .then(() => setError(""))
      .catch((e) => setError(errorMessage(e)));

  return (
    <>
      <PageHeader title="Screen" />
      <div className="space-y-4">
        {error && <Alert tone="red">{error}</Alert>}
        {!mirrorAvailable && <Alert tone="blue" title="Screen mirroring is unavailable." />}
        <Card>
          {panel && <Phone panel={panel} model={model} keys={keys} sideSize={side} touch={touch ? main : undefined} />}
          {unplaced.length > 0 && (
            <div className="mx-auto mt-4 flex max-w-xl flex-wrap gap-2">
              {unplaced.map((k) => (
                <button key={keyId(k)} type="button" className="rounded-md bg-slate-200 px-3 py-2 text-xs font-medium dark:bg-slate-700" onClick={() => tap(k)}>
                  {keyName(k)}
                </button>
              ))}
            </div>
          )}
        </Card>
      </div>
    </>
  );
}
