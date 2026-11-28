# The built-in phone apps.  `apps/` is its own cargo workspace (excluded from
# the firmware workspace); its crates are plain cdylibs for
# wasm32-unknown-unknown, each with its manifest and its data files in the
# module (`export_app!` of the SDK).  `opendeskphone-apps-build stage` then
# makes a component of each module, as `wasm-tools component new` does for an
# app that is installed, compiles it to Pulley bytecode for the phone, and lays
# out
# <name>/{app.pb,<name>.cwasm,data files}, which the image installs as
# /usr/share/opendeskphone/apps.
#
# The compiled code is tied to the Wasmtime version and engine settings of
# `crates/opendeskphone-apps`, so the staging tool comes from the same source as
# the phone binary (`opendeskphone-apps-build` in workspace.nix).
{
  lib,
  runCommand,
  rustPlatform,
  rust,
  opendeskphone-apps-build,
}:
let
  root = ../.;
  wasmTarget = "wasm32-unknown-unknown";
  # One cargo workspace of apps (`dir`, relative to the repository root):
  # its source, and its dependencies as its own Cargo.lock pins them.
  workspaceOf = dir: {
    version = "3.1.0";
    src = lib.fileset.toSource {
      inherit root;
      fileset = lib.fileset.unions [
        (root + "/${dir}")
        # The SDK (with the interface definition) inherits its package
        # fields from the firmware workspace.  Its feature `glyph` needs
        # the macro crate and the renderer.
        (root + "/Cargo.toml")
        (root + "/crates/opendeskphone-app-sdk")
        (root + "/crates/opendeskphone-app-sdk-macros")
        (root + "/crates/opendeskphone-svg")
        # The fonts of the Timer app's digits, made into tables as it builds.
        (root + "/assets/fonts/Roboto-Thin.ttf")
        (root + "/assets/fonts/bitmap/helvB24-100dpi.bdf")
      ];
    };
    cargoRoot = dir;
    cargoDeps = rustPlatform.importCargoLock { lockFile = root + "/${dir}/Cargo.lock"; };
  };
  # Build one cargo workspace of apps to WebAssembly modules, flat, under
  # $out/.
  wasmModules =
    { pname, dir }:
    rust.buildCargo (
      workspaceOf dir
      // {
        inherit pname;
        env.CARGO_BUILD_TARGET = wasmTarget;
        target = wasmTarget;
        commands = [
          [
            "build"
            "--release"
            "--workspace"
          ]
        ];
        artifacts = [ "*.wasm" ];
      }
    );
  modules = wasmModules {
    pname = "opendeskphone-apps-wasm";
    dir = "apps";
  };

  # The apps' own tests (each app crate's unit tests, and those of the
  # games' libraries: the rules, levels, solver, and generator), run on
  # the build machine with the same command as locally (`cargo host-test`
  # in `apps/`, an alias in `apps/.cargo/config.toml`).  The firmware
  # workspace's `cargo test` does not reach them, because `apps/` is a
  # workspace of its own; the `tests` check requires this derivation.
  unitTests = rust.buildCargo (
    workspaceOf "apps"
    // {
      pname = "opendeskphone-apps-unit-tests";
      commands = [ [ "host-test" ] ];
    }
  );
  # The lints, likewise: the check `clippy` requires this derivation.
  clippy = rust.buildCargo (
    workspaceOf "apps"
    // {
      pname = "opendeskphone-apps-clippy";
      commands = [
        [
          "clippy"
          "--target"
          "host-tuple"
          "--all-targets"
          "--"
          "-D"
          "warnings"
        ]
      ];
    }
  );

  # What the host tests of `crates/opendeskphone-apps` run: the built-in apps
  # and the test-only guests, compiled for the build machine's interpreter.
  # The tests otherwise build these themselves with cargo, which a sandboxed
  # build cannot do (no network for the apps' dependencies); the `tests` check
  # hands them this directory as test-inputs/apps-staged.
  guests = "crates/opendeskphone-apps/tests/guests";
  testFixture =
    runCommand "opendeskphone-apps-test-fixture-3.1.0" { nativeBuildInputs = [ opendeskphone-apps-build ]; }
      ''
        opendeskphone-apps-build stage \
          --wasm-dir ${modules} \
          --out $out \
          --target pulley64
        opendeskphone-apps-build stage \
          --wasm-dir ${
            wasmModules {
              pname = "opendeskphone-apps-test-guests-wasm";
              dir = guests;
            }
          } \
          --out $out \
          --target pulley64
      '';
in
runCommand "opendeskphone-apps-3.1.0"
  {
    nativeBuildInputs = [ opendeskphone-apps-build ];
    passthru = {
      inherit
        modules
        testFixture
        unitTests
        clippy
        ;
    };
  }
  ''
    opendeskphone-apps-build stage \
      --wasm-dir ${modules} \
      --out $out \
      --target phone --no-wasm
  ''
