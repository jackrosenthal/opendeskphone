// Drawings of each model's front panel, as data for pages/Screen.tsx.
//
// Reference material (kept outside the repository), by model:
// - D40, D45: retailer top-view and front photographs of a D40; Digium
//   "Welcome to the Digium D40 IP Phone" quick reference (613-00018B)
//   drawing; the D40/D45 photograph in the Digium phones datasheet.  No
//   photograph of a D45 by itself was found; the datasheet shows one body
//   for both.
// - D50: retailer front photograph and the datasheet photograph.
// - D70: retailer front photograph of a D70 with text keys, and photographs
//   at an angle of D70s with icon keys.
// - D60, D62: retailer front photograph of a D60; retailer photograph of a
//   D62 with its lamps lit.
// - D65: retailer top-view photographs, with and without the handset;
//   "Digium D6X Series IP Phone Quick Start Guide" (613-00026) drawing.
// - D80: none.  The D80's drawing is schematic until references are
//   added: an upright touchscreen (800x1280) in a plain black body, the
//   handset in a cradle at the body's left, and no keys and no lamps (the
//   D80 has neither).  Its proportions, its colors, and where its handset
//   is are not the phone's.
// - Sizes: the datasheet's displays (D40, D45, D50: 3.5 inch, 240x120;
//   D70: 4.5 inch, 320x160) and the phones' widths (D40/D45 8.25 inches,
//   D50 10.5 inches, D70 12.5 inches).
//
// What those show, and this file follows:
// - Old family (D40, D45, D50, D70): charcoal body; a silver display
//   housing with rounded corners that rises above the body's top edge;
//   teal soft keys and silver line keys on the housing, each joined to the
//   display by a printed line; silver headset, speaker, and mute keys
//   carrying icons drawn turned; black dial and feature keys.  D40/D45/D50
//   feature keys carry words (Msgs, Transfer, Redial, Hold, Conf, Info);
//   the D70's carry icons.  The check key is the center of the navigation
//   pad.  The D40, D45, and D50 share one housing; the D70's is larger, for
//   the D70's larger display.
// - New family (D60, D62, D65): black, in two panels: a glossy upper panel
//   with the display, the silver line and soft keys, and the logo; a matte
//   lower panel with the keys.  Every legend is an icon.  The navigation
//   pad is one disc with no center key; check and X share a bar above it.
// - Both families: the dial keys (and the old family's headset, speaker,
//   and mute keys) have straight sides and top and bottom edges that bow
//   outward.  Each digit stands below the space for letters, also on the
//   1 key, which has no letters; the star and pound sit in the middle of
//   their keys, and so does the new family's 0.  The old family's 0 has
//   OPER above it, placed like the other digits.  A handset in a legend is
//   a C-shaped handle with a separate cap at each end.
//
// Not established from the references: the printed lines from the line
// keys to the display on the new family (none is visible, so none is
// drawn); a D62's page key (drawn without, like the D60).
//
// Every phone is drawn flat, as seen straight on: where the only
// photograph is at an angle (D50, D70), positions follow the front-on
// drawings and the family's common grid.  No maker's name or mark is
// drawn.
//
// Units are those of the reference photograph each family was measured
// from; the page scales the drawing.

import { Model } from "../gen/opendeskphone/v1/model_pb";
import { Key_Kind as K, type Key } from "../gen/opendeskphone/v1/screen_pb";

/// A key as the ScreenService names it: its kind, and its index where the
/// kind has several keys.
export type KeyId = Pick<Key, "kind" | "index" | "module">;

/// The key of a kind (and of an index, where the kind has several keys).
export const key = (kind: K, index = 0): KeyId => ({ kind, index, module: 0 });

export const sameKey = (a: KeyId, b: KeyId) => a.kind === b.kind && a.index === b.index && a.module === b.module;

export type Legend =
  | { icon: string }
  | { text: string }
  /// Dial pad: letters above the digit.  A key with `letters` (empty on
  /// the 1 key) has its digit below the space for letters; a key without
  /// has its digit in the middle.
  | { digit: string; letters?: string };

export type PanelKey = {
  key: KeyId;
  cx: number;
  cy: number;
  w: number;
  h: number;
  /// Corner radius; half the height for an oval, large for a round key.
  r: number;
  fill: string;
  /// Legend color.
  ink?: string;
  legend?: Legend;
  /// Legend size, in units.
  size?: number;
  /// The key has a lamp behind it, which lights the whole key.
  lamp?: boolean;
  /// No outline of its own: part of a larger drawn control (the new
  /// family's navigation disc; halves of a bar).
  bare?: boolean;
  /// The top and bottom edges bow outward by this much at the middle; the
  /// sides are straight, and `r` rounds the corners.
  arch?: number;
};

export type Shape = { d: string; fill: string; stroke?: string; opacity?: number; gloss?: boolean };

export type Panel = {
  viewBox: [number, number, number, number];
  /// Factor from units to CSS pixels at which the display is 1:1 with the
  /// phone's own pixels times `pixelScale`.
  unit: number;
  /// The page keeps the whole drawing within the window's height (D80,
  /// whose screen stands upright and would otherwise run off the bottom).
  fitHeight?: boolean;
  /// The picture is smoothed when scaled, not drawn as blocks of pixels
  /// (D80, whose screen is shown smaller than its pixels).
  smooth?: boolean;
  /// Body parts, back to front, under the keys.
  shapes: Shape[];
  /// Printed guide lines: x1, y1, x2, y2.
  lines: [number, number, number, number][];
  lineColor: string;
  display: { x: number; y: number; w: number; h: number };
  side?: { x: number; y: number; w: number; h: number };
  /// Message waiting lamp: outline, and its color when dark.
  mwi?: { d: string; off: string };
  handset: { x: number; y: number; w: number; h: number; fill: string; print?: string };
  /// Speaker grille in the cradle, under the handset.
  grille?: { cx: number; cy: number; rx: number; ry: number };
  keys: PanelKey[];
};

const rect = (x: number, y: number, w: number, h: number, r: number) =>
  `M${x + r} ${y}H${x + w - r}Q${x + w} ${y} ${x + w} ${y + r}V${y + h - r}Q${x + w} ${y + h} ${x + w - r} ${y + h}H${x + r}Q${x} ${y + h} ${x} ${y + h - r}V${y + r}Q${x} ${y} ${x + r} ${y}Z`;

/// Outline of a key with straight sides and top and bottom edges that bow
/// outward by `a` at the middle (each edge a parabola), within the box
/// (x, y, w, h), with corners rounded by `r`.
export function archedRect(x: number, y: number, w: number, h: number, r: number, a: number): string {
  // The top edge from (x, y + a) through (x + w/2, y) to (x + w, y + a):
  // a quadratic with its control point at (x + w/2, y - a).  The part
  // between t0 and t1 is again a quadratic.
  const t0 = r / w;
  const t1 = 1 - t0;
  const edge = (yEnd: number, yCtl: number) => {
    const at = (t: number) => [x + w * t, (1 - t) * (1 - t) * yEnd + 2 * t * (1 - t) * yCtl + t * t * yEnd];
    const slope = (t: number) => 2 * (1 - t) * (yCtl - yEnd) + 2 * t * (yEnd - yCtl);
    const [ax, ay] = at(t0);
    const [bx, by] = at(t1);
    const c = [ax + ((t1 - t0) * w) / 2, ay + ((t1 - t0) * slope(t0)) / 2];
    return { ax, ay, bx, by, c };
  };
  const top = edge(y + a, y - a);
  const bot = edge(y + h - a, y + h + a);
  return (
    `M${x} ${y + a + r}Q${x} ${y + a} ${top.ax} ${top.ay}Q${top.c[0]} ${top.c[1]} ${top.bx} ${top.by}Q${x + w} ${y + a} ${x + w} ${y + a + r}` +
    `V${y + h - a - r}Q${x + w} ${y + h - a} ${bot.bx} ${bot.by}Q${bot.c[0]} ${bot.c[1]} ${bot.ax} ${bot.ay}Q${x} ${y + h - a} ${x} ${y + h - a - r}Z`
  );
}

/// The letters on each dial key; the 1 key has a row for letters with none
/// in it, and the 0 (new family), star, and pound keys have no row.
const letters: Record<string, string> = { "1": "", "2": "ABC", "3": "DEF", "4": "GHI", "5": "JKL", "6": "MNO", "7": "PQRS", "8": "TUV", "9": "WXYZ" };
const digits = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "*", "0", "#"];
/// The key of a dial-pad legend.
const dialKey = (d: string): KeyId => (d === "*" ? key(K.STAR) : d === "#" ? key(K.POUND) : key(K.DIGIT, Number(d)));

/// A dial key's legend: the star is drawn, centered; the others are type.
/// The old family prints OPER above the 0.
const dialLegend = (d: string, old = false): Legend =>
  d === "*" ? { icon: "asterisk" } : { digit: d, letters: old && d === "0" ? "OPER" : letters[d] };

// ------------------------------------------------------------ new family

const BLACK = "#17181a";
const SILVER = "#d3d6d9";

function newFamily(lineKeys: number, pageKey: boolean): Panel {
  const keys: PanelKey[] = [];
  const white = "#f2f3f4";

  // Line keys: silver ovals right of the display, staggered.
  const linePos = [
    [655, 168],
    [697, 190],
    [655, 215],
    [697, 240],
    [655, 265],
    [697, 289],
  ];
  for (let i = 0; i < lineKeys; i++) {
    keys.push({ key: key(K.LINE, i + 1), cx: linePos[i][0], cy: linePos[i][1], w: 30, h: 16, r: 8, fill: SILVER, lamp: true });
  }
  // D65: one silver rocker under the line keys, as wide as the two columns
  // of line keys together, pages the rapid-dial keys: the left half is the
  // previous page, the right half the next.  The halves are drawn as one
  // control (`rocker`, below); each half is a key of its own.
  const rocker = { x0: Math.min(...linePos.map((p) => p[0])) - 15, x1: Math.max(...linePos.map((p) => p[0])) + 15, cy: 327, h: 16 };
  const rockerMid = (rocker.x0 + rocker.x1) / 2;
  if (pageKey) {
    keys.push({ key: key(K.BLF_PAGE_LEFT), cx: (rocker.x0 + rockerMid) / 2, cy: rocker.cy, w: rockerMid - rocker.x0, h: rocker.h, r: 6, fill: SILVER, bare: true });
    keys.push({ key: key(K.BLF_PAGE_RIGHT), cx: (rockerMid + rocker.x1) / 2, cy: rocker.cy, w: rocker.x1 - rockerMid, h: rocker.h, r: 6, fill: SILVER, bare: true });
  }
  // The rocker's print: a triangle toward each end (the icons page-left and
  // page-right, 11 units, 11 units in from the end) and a fine line across
  // the middle, where the rocker pivots.
  const triangle = (cx: number, pts: [number, number][]) =>
    "M" + pts.map(([x, y]) => `${cx - 5.5 + (x * 11) / 24} ${rocker.cy - 5.5 + (y * 11) / 24}`).join("L") + "Z";
  const rockerShapes: Shape[] = pageKey
    ? [
        { d: rect(rocker.x0, rocker.cy - rocker.h / 2, rocker.x1 - rocker.x0, rocker.h, 6), fill: SILVER, stroke: "#00000066" },
        { d: `M${rockerMid} ${rocker.cy - rocker.h / 2}V${rocker.cy + rocker.h / 2}`, fill: "none", stroke: "#9aa1a8" },
        { d: triangle(rocker.x0 + 11, [[17, 4.5], [6, 12], [17, 19.5]]), fill: "#69727d" },
        { d: triangle(rocker.x1 - 11, [[7, 4.5], [18, 12], [7, 19.5]]), fill: "#69727d" },
      ]
    : [];

  // Soft keys: silver ovals under the display.
  const softX = [355, 427, 500, 573];
  softX.forEach((cx, i) => keys.push({ key: key(K.SOFT, i + 1), cx, cy: 378, w: 42, h: 15, r: 7.5, fill: SILVER }));

  // Headset, speaker, mute: silver ovals with gray icons.
  const audio: [K, string, number][] = [
    [K.HEADSET, "headset", 462],
    [K.SPEAKER, "speaker", 514],
    [K.MUTE, "mute", 566],
  ];
  audio.forEach(([kind, icon, cy]) =>
    keys.push({ key: key(kind), cx: 327, cy, w: 46, h: 22, r: 11, fill: SILVER, ink: "#69727d", legend: { icon }, size: 18, lamp: true }),
  );

  // Dial pad: black keys, white print.
  digits.forEach((d, i) =>
    keys.push({ key: dialKey(d), cx: 409 + (i % 3) * 59, cy: 462 + Math.floor(i / 3) * 51.5, w: 46, h: 35, r: 3, arch: 1.9, fill: BLACK, ink: white, legend: dialLegend(d), size: 15.5 }),
  );
  // Volume: one black bar under the dial pad.
  keys.push({ key: key(K.VOLUME_DOWN), cx: 438, cy: 670, w: 62, h: 20, r: 3, fill: BLACK, ink: white, legend: { icon: "minus" }, size: 14, bare: true });
  keys.push({ key: key(K.VOLUME_UP), cx: 500, cy: 670, w: 62, h: 20, r: 3, fill: BLACK, ink: white, legend: { icon: "plus" }, size: 14, bare: true });

  // Check and X: the two ends of one bar above the pad.
  keys.push({ key: key(K.SELECT), cx: 623, cy: 446, w: 46, h: 20, r: 3, fill: BLACK, ink: white, legend: { icon: "check" }, size: 13, bare: true });
  keys.push({ key: key(K.CANCEL), cx: 669, cy: 446, w: 46, h: 20, r: 3, fill: BLACK, ink: white, legend: { icon: "x" }, size: 12, bare: true });
  // Navigation: one disc, four directions, no center key.
  const nav: [K, string, number, number][] = [
    [K.UP, "up", 648, 490],
    [K.LEFT, "left", 624, 514],
    [K.RIGHT, "right", 672, 514],
    [K.DOWN, "down", 648, 538],
  ];
  nav.forEach(([kind, icon, cx, cy]) => keys.push({ key: key(kind), cx, cy, w: 26, h: 26, r: 13, fill: BLACK, ink: white, legend: { icon }, size: 14, bare: true }));
  // Messages, transfer, redial, hold: black ovals, white icons.
  const feat: [K, string, number, number][] = [
    [K.MESSAGES, "msgs", 610, 590],
    [K.TRANSFER, "xfer", 688, 590],
    [K.REDIAL, "redial", 610, 637],
    [K.HOLD, "hold", 688, 637],
  ];
  feat.forEach(([kind, icon, cx, cy]) => keys.push({ key: key(kind), cx, cy, w: 56, h: 25, r: 12.5, fill: BLACK, ink: white, legend: { icon }, size: 17 }));

  return {
    viewBox: [-176, 44, 950, 760],
    unit: 480 / 303,
    shapes: [
      // Cradle wing, left of the two panels; vertical outer edge.
      { d: "M276 170H88Q62 170 62 196V674Q62 700 88 700H276Z", fill: "#2b2d30" },
      // Upper cup the earpiece rests in, with the hook switch.
      { d: "M92 172H234V226Q234 264 196 264H130Q92 264 92 226Z", fill: "#1c1d20" },
      { d: rect(152, 246, 22, 9, 2), fill: "#0c0d0e" },
      // Lower arch the mouthpiece rests in.
      { d: "M98 700V642Q98 568 164 568Q230 568 230 642V700Z", fill: "#1c1d20" },
      // Upper panel: glossy black.
      { d: "M290 112H736Q758 112 758 134V412H268V134Q268 112 290 112Z", fill: "#0e0f11", gloss: true },
      // Lower panel: matte.
      { d: "M268 410H758V693Q758 715 736 715H290Q268 715 268 693Z", fill: "#2a2c2f" },
      // The page rocker, drawn whole on every D65; each half works where
      // the phone reports that half's key.
      ...rockerShapes,
      // The joined bars and the navigation disc.
      { d: rect(407, 660, 124, 20, 10), fill: BLACK, stroke: "#050506" },
      { d: "M469 662V678", fill: "none", stroke: "#3a3c40" },
      { d: rect(600, 436, 92, 20, 10), fill: BLACK, stroke: "#050506" },
      { d: "M646 438V454", fill: "none", stroke: "#3a3c40" },
      { d: "M648 477a37 37 0 1 0 .01 0Z", fill: BLACK, stroke: "#050506" },
    ],
    // A printed tick above each soft key, up to the display.
    lines: softX.map((cx) => [cx, 334, cx, 366]),
    lineColor: "#5d6066",
    display: { x: 312, y: 155, w: 303, h: 303 * (272 / 480) },
    // A strip in the top edge of the upper panel, near the right corner.
    mwi: { d: rect(678, 112, 28, 6, 1), off: "#b9bdc2" },
    handset: { x: 50, y: 63, w: 174, h: 669, fill: "#222326" },
    grille: { cx: 164, cy: 410, rx: 62, ry: 62 },
    keys,
  };
}

// ------------------------------------------------------------ old family

const CHARCOAL = "#34373c";
const KEY = "#17181b";
const HOUSING_KEY = "#eceef0";
const TEAL = "#5f9db8";
const OLD_SILVER = "#d5d8db";
/// The D50's label card: rows alternately white and solid light gray.
const PAPER_WHITE = "#fbfbfc";
const PAPER_GRAY = "#c4c8cd";

/// A key and the name of its legend: an icon, or a word in `words`.
type Labeled = [K, string];

type Old = {
  lineKeys: number;
  /// Feature key rows under the pad; three columns on the D50 and D70.
  features: Labeled[][];
  /// Words (D40, D45, D50) or icons (D70) on the feature keys.
  words: boolean;
  top?: Labeled[];
  beside?: Labeled;
  wing?: "paper" | "display";
};

const words: Record<string, string> = { msgs: "Msgs", xfer: "Transfer", redial: "Redial", hold: "Hold", conf: "Conf", info: "Info" };

/// Where the parts of each body are, from the front photographs.  The
/// housing's left edge is at 300 on every model.
type OldLayout = {
  /// The handset (x, y, width, length) and the cradle's top and bottom
  /// edges.
  handset: [number, number, number, number];
  cradle: [number, number];
  /// The body: left edge, top edge, right edge, the bottom corners' height, and how far the bottom edge
  /// bows down between them.  `wingRise`: how much higher the rapid-dial
  /// section's bottom-right corner is; `wingDrop`: how much lower the
  /// section's top-right corner is.
  body: { xl: number; top: number; xr: number; yb: number; bow: number; wingRise?: number; wingDrop?: number };
  /// The housing: right edge; how much higher the top corners are than
  /// the D40's; how much the top edge arches; and the bottom edge, at the
  /// ends of the corners and (the curve's control point) in the middle.
  housing: { hr: number; lift: number; arch: number; y1: number; yc: number };
  display: { x: number; y: number; w: number; h: number };
  bezel: [number, number, number, number];
  /// Soft keys: height and size.
  soft: [number, number, number];
  /// The two columns of line keys, and the first key's height and the step.
  lineX: [number, number];
  lineY?: [number, number];
  /// Headset, speaker, and mute: x, first y, step, size.
  side: [number, number, number, number, number];
  /// Dial pad: first key's middle, pitch across and down, key size.
  dial: [number, number, number, number, number, number];
  /// Volume bar: middle and width.
  volume: [number, number, number];
  /// Columns of the keys right of the pad.
  fx: number[];
  /// Top row (D70): height and key size.
  top?: [number, number, number];
  /// Navigation pad: middle, step from the middle to the arrows (across,
  /// down), the
  /// cancel key's offset from the middle, key size, and ring radius.
  nav: { x: number; y: number; step: [number, number]; cancel: [number, number]; size: number; ring: number };
  /// The key beside the pad (D70): middle.
  beside?: [number, number];
  /// Feature keys: first row's height, step, key size.
  feature: [number, number, number, number];
  /// The rapid-dial section (D70): the second display's x offset from the
  /// housing, top, and width; the keys' x offset and size; the page keys'
  /// distance below the display and size.
  wing?: { dx: number; y: number; w: number; keyX: number; key: [number, number]; pageY: number; page: [number, number]; pageX: [number, number] };
};

const D40_DISPLAY = { x: 340, y: 200, w: 196, h: 98 };

const layouts: Record<"D40" | "D50" | "D70", OldLayout> = {
  // The body's sides are close to the housing's.
  D40: {
    handset: [106, 96, 152, 512], cradle: [232, 598],
    body: { xl: 284, top: 212, xr: 680, yb: 598, bow: 16 },
    housing: { hr: 664, lift: 0, arch: 6, y1: 348, yc: 378 },
    display: D40_DISPLAY, bezel: [326, 188, 224, 122], soft: [338, 36, 15], lineX: [577, 610],
    side: [334, 402, 50, 37, 28], dial: [394, 402, 47, 40, 37, 29], volume: [442, 566, 112],
    fx: [568, 636], nav: { x: 590, y: 430, step: [32, 30], cancel: [34, -32], size: 23, ring: 40 }, feature: [503, 40, 48, 24],
  },
  // The D40's housing.  The keys sit further left under the housing than
  // on the D40, to make room for three columns of smaller feature keys,
  // the middle column under the navigation pad.
  D50: {
    handset: [106, 96, 152, 512], cradle: [232, 598],
    body: { xl: 279, top: 212, xr: 853, yb: 598, bow: 16, wingRise: 14 },
    housing: { hr: 664, lift: 0, arch: 6, y1: 348, yc: 378 },
    display: D40_DISPLAY, bezel: [326, 188, 224, 122], soft: [338, 36, 15], lineX: [577, 610],
    side: [313, 402, 50, 37, 28], dial: [373, 402, 47, 40, 37, 29], volume: [421, 566, 112],
    fx: [528, 589, 650], nav: { x: 589, y: 430, step: [32, 30], cancel: [34, -32], size: 23, ring: 40 }, feature: [503, 40, 44, 20],
  },
  // From a front photograph of a D70 (one photograph pixel is 1.083
  // units): a 4.5-inch display (the D40's: 3.5 inches) in a larger housing
  // that rises higher, with a stronger arch; square dial keys; the
  // navigation pad's middle halfway between the first and second columns
  // of keys right of it; a second display and small rapid-dial keys in a
  // section whose top falls toward the right.
  D70: {
    handset: [124.5, 28, 145, 576], cradle: [127, 526.5],
    body: { xl: 284, top: 107, xr: 995.4, yb: 584, bow: 46, wingRise: 0, wingDrop: 17 },
    housing: { hr: 799.4, lift: 86.3, arch: 18.4, y1: 326, yc: 387 },
    display: { x: 359.5, y: 139, w: 282, h: 141 }, bezel: [337, 115, 321, 179], soft: [312.6, 38, 17],
    lineX: [685.6, 720.3], lineY: [144, 20.4],
    side: [351.4, 404, 56, 36, 26], dial: [425, 402.5, 47.5, 45, 35, 33], volume: [470.5, 586, 103],
    fx: [594, 658, 723], top: [384.6, 42, 22], nav: { x: 626, y: 460.5, step: [35, 35], cancel: [36.8, -35.7], size: 20, ring: 44.7 },
    beside: [723, 491.3], feature: [531, 40, 42, 23],
    wing: { dx: 16.2, y: 210.7, w: 103, keyX: 150.6, key: [25, 13], pageY: 28.5, page: [36, 16], pageX: [0.24, 0.74] },
  },
};

function oldFamily(v: Old, L: OldLayout): Panel {
  const keys: PanelKey[] = [];
  const white = "#f2f3f4";
  const { display: d } = L;
  const { hr, lift } = L.housing;
  const [bx, by, bw, bh] = L.bezel;
  const [hsx, hsy, hsw, hsh] = L.handset;

  // Line keys on the housing, right of the display, staggered; a printed
  // line runs from each to the display bezel.
  const lines: [number, number, number, number][] = [];
  const [y0, step] = L.lineY ?? [v.lineKeys > 2 ? 204 : 214, v.lineKeys > 4 ? 15 : v.lineKeys > 2 ? 19 : 17];
  for (let i = 0; i < v.lineKeys; i++) {
    const cx = L.lineX[i % 2];
    const cy = y0 + i * step;
    keys.push({ key: key(K.LINE, i + 1), cx, cy, w: 27, h: 14, r: 7, fill: HOUSING_KEY, lamp: true });
    lines.push([bx + bw, cy, cx - 13.5, cy]);
  }

  // Soft keys: teal, under the display's quarters, each with a line up to
  // the bezel.
  const [softY, softW, softH] = L.soft;
  const softX = [0, 1, 2, 3].map((i) => d.x + (d.w * (i + 0.5)) / 4);
  softX.forEach((cx, i) => {
    keys.push({ key: key(K.SOFT, i + 1), cx, cy: softY, w: softW, h: softH, r: 5, fill: TEAL });
    lines.push([cx, by + bh - 2, cx, softY - softH / 2]);
  });

  // Headset, speaker, mute: silver keys, shaped like the dial keys, with
  // dark icons.
  const [sx, sy0, sstep, sw, sh] = L.side;
  const audio: Labeled[] = [
    [K.HEADSET, "headset-turned"],
    [K.SPEAKER, "speaker-turned"],
    [K.MUTE, "mute-turned"],
  ];
  audio.forEach(([kind, icon], i) =>
    keys.push({ key: key(kind), cx: sx, cy: sy0 + i * sstep, w: sw, h: sh, r: 3.5, arch: 1.3, fill: OLD_SILVER, ink: "#555b63", legend: { icon }, size: 20, lamp: true }),
  );

  // Dial pad.
  const [dx0, dy0, ddx, ddy, dw, dh] = L.dial;
  digits.forEach((dg, i) =>
    keys.push({ key: dialKey(dg), cx: dx0 + (i % 3) * ddx, cy: dy0 + Math.floor(i / 3) * ddy, w: dw, h: dh, r: 3, arch: 1.4, fill: KEY, ink: white, legend: dialLegend(dg, true), size: 12.9 }),
  );
  // Volume: one bar.
  const [vx, vy, vw] = L.volume;
  keys.push({ key: key(K.VOLUME_DOWN), cx: vx - vw / 4, cy: vy, w: vw / 2, h: 16, r: 3, fill: KEY, ink: white, legend: { icon: "minus" }, size: 11, bare: true });
  keys.push({ key: key(K.VOLUME_UP), cx: vx + vw / 4, cy: vy, w: vw / 2, h: 16, r: 3, fill: KEY, ink: white, legend: { icon: "plus" }, size: 11, bare: true });

  // Right of the dial pad.
  const { fx, nav: n } = L;
  if (v.top && L.top) {
    const [ty, tw, th] = L.top;
    v.top.forEach(([kind, icon], i) => keys.push({ key: key(kind), cx: fx[i], cy: ty, w: tw, h: th, r: 4, fill: KEY, ink: white, legend: { icon }, size: 16 }));
  }
  // Navigation: five round keys, check in the center; X above and right.
  const nav: [K, string, number, number][] = [
    [K.UP, "up", n.x, n.y - n.step[1]],
    [K.CANCEL, "x", n.x + n.cancel[0], n.y + n.cancel[1]],
    [K.LEFT, "left", n.x - n.step[0], n.y],
    [K.SELECT, "check", n.x, n.y],
    [K.RIGHT, "right", n.x + n.step[0], n.y],
    [K.DOWN, "down", n.x, n.y + n.step[1]],
  ];
  nav.forEach(([kind, icon, cx, cy]) => keys.push({ key: key(kind), cx, cy, w: n.size, h: n.size, r: n.size / 2, fill: KEY, ink: white, legend: { icon }, size: (11 * n.size) / 23 }));
  if (v.beside && L.beside && L.top) keys.push({ key: key(v.beside[0]), cx: L.beside[0], cy: L.beside[1], w: L.top[1], h: L.top[2], r: 4, fill: KEY, ink: white, legend: { icon: v.beside[1] }, size: 16 });
  const [fy, fstep, fw, fh] = L.feature;
  v.features.forEach((row, r) =>
    row.forEach(([kind, id], i) =>
      keys.push({
        key: key(kind),
        cx: fx[i],
        cy: fy + r * fstep,
        w: fw,
        h: fh,
        r: 4,
        fill: KEY,
        ink: white,
        legend: v.words ? { text: words[id] } : { icon: id },
        size: v.words ? 9.5 : 16,
      }),
    ),
  );

  const hl = 300; // housing left edge
  // The rapid-dial section (D50, D70) starts at the housing's right edge.
  // Outline of body and rapid-dial section together.  The sides are
  // vertical; the bottom edge bows downward.
  const { xl, top: bt, yb, bow } = L.body;
  const xr = L.body.xr;
  const rise = L.body.wingRise ?? 14;
  const drop = L.body.wingDrop ?? 0;
  // Bottom edge: one quadratic from (xl, yb) to (xr, yb), bowed by `bow`.
  // The curve runs between the rounded bottom corners.
  const cl = xl + 20;
  // On the D50 and D70 the curve belongs to the main body alone and ends
  // where the rapid-dial section begins.
  const cr = v.wing ? hr : xr - 20;
  // The bottom edge from `xa` back to the left corner: a quadratic bowed
  // by `bow` between the corners.
  const bottomTo = (xa: number) => {
    const t = (xa - cl) / (cr - cl);
    const cy = yb + 2 * bow * t;
    return `Q${(cl + xa) / 2} ${cy} ${cl} ${yb}Q${xl} ${yb} ${xl} ${yb - 20}`;
  };
  const [ct, cb] = L.cradle;
  const cx0 = hsx - 8;
  const shapes: Shape[] = [
    // Cradle wing under the handset.
    { d: rect(cx0, ct, 200, cb - ct, 24), fill: "#2b2e32" },
    // Upper cup the earpiece rests in, with the hook switch.
    { d: rect(cx0 + 18, ct + 12, 136, 72, 24), fill: "#1f2124" },
    { d: rect(cx0 + 76, ct + 64, 20, 9, 2), fill: "#0e0f10" },
    // Lower cup the mouthpiece rests in.
    { d: rect(cx0 + 18, cb - 100, 136, 88, 28), fill: "#1f2124" },
    // Body.
    {
      d: v.wing
        ? // Body and rapid-dial section as one outline: the section's
          // bottom edge is straight, from the end of the body's curve to a
          // rounded bottom-right corner.
          `M${xl + 24} ${bt}H${hr}L${xr - 24} ${bt + drop}Q${xr} ${bt + drop} ${xr} ${bt + drop + 24}V${yb - rise - 24}Q${xr} ${yb - rise - 2} ${xr - 22} ${yb - rise}L${hr} ${yb}${bottomTo(hr)}V${bt + 24}Q${xl} ${bt} ${xl + 24} ${bt}Z`
        : `M${xl + 24} ${bt}H${xr - 24}Q${xr} ${bt} ${xr} ${bt + 24}V${yb - 20}Q${xr} ${yb} ${cr} ${yb}${bottomTo(cr)}V${bt + 24}Q${xl} ${bt} ${xl + 24} ${bt}Z`,
      fill: CHARCOAL,
    },
  ];
  // Rapid-dial section: part of the body's own surface (drawn in the body
  // outline above), directly right of the silver housing, holding ten
  // keys beside a label window (D50) or a second display (D70).
  let side: Panel["side"];
  let vbRight = xr + 28;
  if (v.wing) {
    const wx = hr;
    if (v.wing === "paper") {
      // D50: ten label rows behind a narrow window, alternately white and
      // solid light gray, a small key beside each row.
      const rowH = 24.3;
      const wy = 268;
      shapes.push({ d: rect(wx + 20, wy - 3, 90, 10 * rowH + 6, 2), fill: "#08090a" });
      for (let i = 0; i < 10; i++) {
        shapes.push({ d: rect(wx + 23, wy + i * rowH, 84, rowH, 0), fill: i % 2 ? PAPER_GRAY : PAPER_WHITE });
        keys.push({ key: key(K.BLF, i + 1), cx: wx + 139, cy: wy + (i + 0.5) * rowH, w: 27, h: 14, r: 5.5, fill: HOUSING_KEY, lamp: true });
      }
    } else if (L.wing) {
      // D70: second display (ten entries and a page row), a key beside
      // each entry, two teal page keys under the display.
      const g = L.wing;
      const sw = g.w;
      const sh = sw * (357 / 113);
      const sy = g.y;
      side = { x: wx + g.dx, y: sy, w: sw, h: sh };
      shapes.push({ d: rect(wx + g.dx - 4, sy - 4, sw + 8, sh + 8, 2), fill: "#08090a" });
      for (let i = 0; i < 10; i++) {
        keys.push({ key: key(K.BLF, i + 1), cx: wx + g.keyX, cy: sy + (i + 0.5) * (sh / 11), w: g.key[0], h: g.key[1], r: g.key[1] / 2, fill: HOUSING_KEY, lamp: true });
      }
      const pages: [K, number][] = [
        [K.BLF_PAGE_LEFT, wx + g.dx + sw * g.pageX[0]],
        [K.BLF_PAGE_RIGHT, wx + g.dx + sw * g.pageX[1]],
      ];
      pages.forEach(([kind, cx]) => {
        keys.push({ key: key(kind), cx, cy: sy + sh + g.pageY, w: g.page[0], h: g.page[1], r: 5, fill: TEAL });
        lines.push([cx, sy + sh + 5, cx, sy + sh + g.pageY - g.page[1] / 2]);
      });
    }
    vbRight = xr + 24;
  }
  const mid = (hl + hr) / 2;
  // The housing's bottom edge bows downward between rounded corners: a
  // quadratic from (hr - rb, y1) to (hl + rb, y1), and at each corner a
  // quadratic whose control point is where the side meets the bottom
  // edge's tangent, so that the corner joins both smoothly.
  const rb = 20;
  const { y1, yc, arch } = L.housing;
  const yk = y1 - (rb * (yc - y1)) / (hr - rb - mid);
  const T = (y: number) => y - lift;
  const nv = L.nav;
  const [vbx, vby] = L.volume;
  shapes.push(
    // Silver display housing, rising above the body's top edge: arched
    // top edge, rounded corners, bowed bottom edge.
    {
      d: `M${hl} ${T(182)}Q${hl} ${T(162)} ${hl + 22} ${T(160)}Q${mid} ${T(160 - 2 * arch)} ${hr - 22} ${T(160)}Q${hr} ${T(162)} ${hr} ${T(182)}V${yk - rb}Q${hr} ${yk} ${hr - rb} ${y1}Q${mid} ${yc} ${hl + rb} ${y1}Q${hl} ${yk} ${hl} ${yk - rb}Z`,
      fill: "#c9ccd0",
      gloss: true,
      stroke: "#9da1a6",
    },
    // Display bezel.
    { d: rect(bx, by, bw, bh, 10), fill: "#2b2d2f" },
    // The volume bar and the ring around the pad.
    { d: rect(vbx - vw / 2, vby - 8, vw, 16, 7), fill: KEY, stroke: "#0b0c0d" },
    { d: `M${vbx} ${vby - 6}V${vby + 6}`, fill: "none", stroke: "#3a3c40" },
    { d: `M${nv.x} ${nv.y - nv.ring}a${nv.ring} ${nv.ring} 0 1 0 .01 0Z`, fill: "none", stroke: "#26282c" },
  );

  const vbTop = Math.min(76, hsy - 20, T(160 - 2 * arch) - 14);
  const vbBottom = hsy + hsh + 84;
  return {
    viewBox: [-98, vbTop, vbRight + 98, vbBottom - vbTop],
    unit: (v.wing === "display" ? 320 : 240) / d.w,
    shapes,
    lines,
    lineColor: "#8b9097",
    display: d,
    side,
    // The lamp is the housing's top-right corner itself: a smoked lens
    // that follows the corner along the top and down the right edge.
    mwi: { d: `M${hr - 38} ${T(159)}L${hr - 22} ${T(160)}Q${hr} ${T(162)} ${hr} ${T(182)}V${T(188)}H${hr - 38}Z`, off: "#8e94a2" },
    handset: { x: hsx, y: hsy, w: hsw, h: hsh, fill: "#303338", print: "HDVoice" },
    // Vertical slots beside the housing, as in the quick reference drawing.
    grille: { cx: cx0 + 86, cy: ct + 168, rx: 40, ry: 58 },
    keys,
  };
}

// ------------------------------------------------------------------- D80

/// The D80, schematic (see the top of this file).  The display is 400 by
/// 640 units, half the screen's pixels each way.
function d80(): Panel {
  const display = { x: 300, y: 96, w: 400, h: 640 };
  const handset = { x: 74, y: 76, w: 150, h: 700, fill: "#222326" };
  return {
    // Room on the left for the handset lifted out of its cradle.
    viewBox: [-114, 26, 874, 834],
    unit: 800 / display.w,
    fitHeight: true,
    smooth: true,
    shapes: [
      // Cradle wing, left of the body.
      { d: rect(62, 110, 210, 640, 26), fill: "#2b2d30" },
      // Upper cup the earpiece rests in, with the hook switch.
      { d: rect(84, 126, 130, 96, 24), fill: "#1c1d20" },
      { d: rect(138, 198, 22, 9, 2), fill: "#0c0d0e" },
      // Lower cup the mouthpiece rests in.
      { d: rect(84, 630, 130, 104, 28), fill: "#1c1d20" },
      // Body: glossy black around the screen.
      { d: rect(266, 50, 468, 760, 30), fill: "#0e0f11", gloss: true },
    ],
    lines: [],
    lineColor: "none",
    display,
    handset,
    keys: [],
  };
}

const two: Labeled[][] = [
  [[K.MESSAGES, "msgs"], [K.TRANSFER, "xfer"]],
  [[K.REDIAL, "redial"], [K.HOLD, "hold"]],
];
const three: Labeled[][] = [
  [[K.MESSAGES, "msgs"], [K.TRANSFER, "xfer"], [K.CONFERENCE, "conf"]],
  [[K.REDIAL, "redial"], [K.HOLD, "hold"], [K.INFO, "info"]],
];

export const panels: Partial<Record<Model, Panel>> = {
  [Model.D40]: oldFamily({ lineKeys: 2, features: two, words: true }, layouts.D40),
  [Model.D45]: oldFamily({ lineKeys: 2, features: two, words: true }, layouts.D40),
  [Model.D50]: oldFamily({ lineKeys: 4, features: three, words: true, wing: "paper" }, layouts.D50),
  [Model.D70]: oldFamily(
    { lineKeys: 6, features: three, words: false, top: [[K.APPS, "apps"], [K.CONTACTS, "dir"], [K.MENU, "menu"]], beside: [K.STATUS, "status"], wing: "display" },
    layouts.D70,
  ),
  [Model.D60]: newFamily(2, false),
  [Model.D62]: newFamily(2, false),
  [Model.D65]: newFamily(6, true),
  [Model.D80]: d80(),
};
