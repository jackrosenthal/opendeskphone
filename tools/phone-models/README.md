# phone-models

Builds the 3D models of the D40, D50, D60, D65, D70, and D80 that the documentation's homepage turns: one binary glTF file per model in `docs/public/phones/`, and `models.json`, which lists the models in the order the homepage shows them.

```sh
cd tools/phone-models
cargo run --release              # writes ../../docs/public/phones/
cargo run --release -- --out DIR
```

The tool is a workspace of its own, as `apps/` is: the tool's dependencies stay out of the firmware's `Cargo.lock` and the Nix build.

## What a model holds

- **Geometry**, generated from each front panel's data (`src/panel.rs`: after `web/src/lib/phones.ts`, corrected against front photographs, and the D80 measured from a front photograph): the body, the display, the keys with rounded edges, the handset, the coiled cord, and the stand.  The stands follow photographs of the stands: the D60, D65, and D80 stand on one tent, one bent sheet of two plates meeting at a ridge a little above the back's middle (the front plate against the back, with two slots; each plate open in its lower part, between two side bars, and ending in a rail along the desk with a rubber foot at each end); the D40, D50, and D70 stand on a base plate with an A-frame on it, two open rectangular frames meeting at a ridge, the front frame against the back.  Every stand is narrower than the body and behind the body's lower edge, so that from the front the body hides the stand.  No maker's name or mark is modeled.
- **The idle screen** on the display, drawn by the phone's own UI (`opendeskphone-ui`) on the HAL simulator with the snapshot test's accounts, keys, and time.  The D60 shows the Tides theme, the D65 the Harbor theme, the D80 the Aurora theme, and the gray displays the LCD's ink and backlight.
- **The key legends**, drawn as SVG (`src/legends.rs`, icons after `web/src/lib/phoneIcons.tsx`) with Roboto Bold from `assets/fonts/`, as one transparent print over the keys.
- **Materials** in glTF's metallic-roughness model, with clear coat on the glossy parts.

Vertices are quantized (`KHR_mesh_quantization`); each model is 0.7 to 0.8 MB.

## The homepage

The documentation site (`docs/`, Astro Starlight) draws the models with three.js: `docs/src/scripts/turntable.ts`, placed on the homepage by `docs/src/components/PhoneTurntable.astro`.  Rebuild the models after a change to a panel's layout in `web/src/lib/phones.ts` or to the idle screen.
