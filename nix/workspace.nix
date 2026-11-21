# The cargo workspace: `opendeskphone` for the phone (one build for the
# D40 thru D70, and one for the D80, with its touch skin), the two host tools
# (`opendeskphone-image`, `opendeskphone-apps-build`), the DPMA module for the
# tests' Asterisk (`res_open_dpma`), and the test and lint checks.
#
# Each product is built in two derivations (crane).  The first compiles only
# the third-party dependencies, from Cargo.lock and the manifests alone, so
# it is rebuilt only when those change.  The second compiles the workspace's
# own crates on top of it.  A change to our code therefore recompiles our
# crates and relinks; it does not recompile the dependencies.
{
  lib,
  craneLib,
  rust,
  rustPlatform,
  protobuf,
  openssh,
  qemu-user,
  asterisk, # Asterisk, its headers, and its modules with ours (asterisk.nix)
  testInputs, # what the tests need that is not in the repository (test-inputs)
  webDist, # built frontend, embedded by opendeskphone-web from web/dist
  appsUnitTests ? null, # the apps' own tests, passed (apps.nix)
  appsClippy ? null, # the apps' own lints, passed (apps.nix)
}:
let
  root = ../.;
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      (root + "/Cargo.toml")
      (root + "/Cargo.lock")
      (root + "/.cargo")
      (root + "/crates")
      (root + "/xtask")
      # The DPMA module for Asterisk and its libraries, two of which the
      # firmware uses too (dpma-proto and dpma-bytes).
      (root + "/open-dpma")
      # The Light Up app's game library, which the host's test of that app
      # (crates/opendeskphone-apps/tests/lightup.rs) links, and the manifest of
      # the apps' workspace, from which it inherits its package fields.
      (root + "/apps/Cargo.toml")
      (root + "/apps/lightup/game")
      # Service definitions, read by the build scripts of opendeskphone-config
      # and opendeskphone-web.
      (root + "/proto")
      # Fonts and SVG artwork, rasterized by opendeskphone-ui's build script.
      (root + "/assets")
    ];
  };
  version = (lib.importTOML (root + "/Cargo.toml")).workspace.package.version;

  common = {
    inherit src version;
    strictDeps = true;
    doCheck = false;
    nativeBuildInputs = [ protobuf ];
    # The hardening wrapper flags are for the native C compiler; they must
    # not leak into the ARM link, which rust-lld does by itself.
    hardeningDisable = [ "all" ];
  };

  # The workspace profile (fat LTO, one codegen unit) is for the phone.  The
  # host tools only run during the build, so they are built the quick way.
  hostEnv = {
    CARGO_PROFILE_RELEASE_LTO = "off";
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16";
    CARGO_PROFILE_RELEASE_OPT_LEVEL = "2";
  };

  # args.cargoExtraArgs selects the packages; the same selection is used for
  # the dependency layer so that feature unification matches.
  twoStage =
    args:
    let
      full = common // args;
      cargoArtifacts = craneLib.buildDepsOnly (full // { pname = "${args.pname}-deps"; });
    in
    {
      deps = cargoArtifacts;
      inherit full;
      package = craneLib.buildPackage (full // { inherit cargoArtifacts; });
    };

  # The firmware binary is built twice, by family of phones.  The D40
  # thru D70 have little flash, so their binary is built
  # without the feature `d80` of crates/opendeskphone: it has nothing of the
  # D80's touch skin, whose fonts and artwork are the largest of any skin.
  # The D80's binary has every feature (cargo's default, which is also what
  # the simulator and the tests are built with), and is built for the D80's
  # own processor (rust.nix), so it has a dependency layer of its own.
  #
  # Each build is checked for the bytes that the touch skin's icon data
  # begins with (`TOUCH_MARK` in crates/opendeskphone-ui/build.rs, which
  # must say the same): the binary of the D40 thru D70 must not have
  # them, and the D80's must, so that neither the skin nor the check goes
  # astray unnoticed.
  touchSkinMark = "opendeskphone-ui touch skin icons";
  phoneArgs = {
    dontStrip = true;
    dontPatchELF = true;
    doInstallCheck = true;
    # The frontend where opendeskphone-web's build script looks for it, as
    # `pnpm build` leaves it in a checkout.
    preBuild = "mkdir -p web && ln -s ${webDist} web/dist";
  };
  phone = twoStage (
    phoneArgs
    // rust.armEnv
    // {
      pname = "opendeskphone";
      cargoExtraArgs = "--locked -p opendeskphone --no-default-features --features web";
      installCheckPhase = ''
        if grep --quiet --text --fixed-strings '${touchSkinMark}' $out/bin/dphone; then
          echo "dphone for the D40 thru D70 has the D80's touch skin: is opendeskphone-ui built with its feature touch?" >&2
          exit 1
        fi
      '';
    }
  );
  phoneD80 = twoStage (
    phoneArgs
    // rust.armv7Env
    // {
      pname = "opendeskphone-d80";
      cargoExtraArgs = "--locked -p opendeskphone";
      # The dependency layer is built with these arguments too, and has
      # no binary to check.
      installCheckPhase = ''
        if test -e $out/bin/dphone && ! grep --quiet --text --fixed-strings '${touchSkinMark}' $out/bin/dphone; then
          echo "dphone for the D80 has no touch skin, or the skin's mark has changed (TOUCH_MARK in crates/opendeskphone-ui/build.rs)" >&2
          exit 1
        fi
      '';
    }
  );

  # The early-boot stage for the kernel's initramfs: no standard library,
  # so it is built to abort on a panic (the workspace profile unwinds), and
  # for size above all.
  early = twoStage (
    {
      pname = "opendeskphone-early";
      cargoExtraArgs = "--locked -p opendeskphone-early";
      dontStrip = true;
      dontPatchELF = true;
      CARGO_PROFILE_RELEASE_PANIC = "abort";
      CARGO_PROFILE_RELEASE_OPT_LEVEL = "z";
    }
    // rust.armEnv
  );

  # The D80's recovery: `/init` of the recovery image's ramdisk, with its
  # pictures (the feature `program`).  It is built like the D80's firmware
  # binary, and it unwinds on a panic, which it catches: a recovery that
  # stops is started again at every power-on.
  recovery = twoStage (
    {
      pname = "opendeskphone-recovery";
      cargoExtraArgs = "--locked -p opendeskphone-recovery --features program";
      dontStrip = true;
      dontPatchELF = true;
    }
    // rust.armv7Env
  );

  imageTool = twoStage (
    {
      pname = "opendeskphone-image";
      cargoExtraArgs = "--locked -p opendeskphone-image";
    }
    // hostEnv
  );

  appsTool = twoStage (
    {
      pname = "opendeskphone-apps-build";
      cargoExtraArgs = "--locked -p opendeskphone-apps --features build-tool --bin opendeskphone-apps-build";
    }
    // hostEnv
  );

  # `res_open_dpma` generates its bindings from the configured headers of the
  # Asterisk it is loaded into (`ASTERISK_SOURCE_DIR`, read by dpma-sys).
  # It has a profile of its own: it runs on a server, not on the phone.
  module = twoStage {
    pname = "res_open_dpma";
    cargoExtraArgs = "--locked -p res_open_dpma";
    CARGO_PROFILE = "module";
    nativeBuildInputs = common.nativeBuildInputs ++ [ rustPlatform.bindgenHook ];
    ASTERISK_SOURCE_DIR = "${asterisk.server}/share/asterisk-source";
  };

  # The crates that need Asterisk's headers are left out: the module's
  # tests cannot link outside Asterisk, and `cargo xtask ci` in open-dpma/
  # builds, lints, and tests all three against every Asterisk release that
  # the module supports (.github/workflows/ci.yml).
  checksStage = twoStage ({
    pname = "opendeskphone-checks";
    cargoExtraArgs = "--locked --workspace --exclude dpma-sys --exclude dpma-ast --exclude res_open_dpma";
    # Debug profile: the checks do not ship.
    CARGO_PROFILE = "dev";
  });
in
{
  inherit src version;

  # The firmware binary of the D40 thru D70: static, ARMv6
  # soft-float, without the D80's touch skin.
  opendeskphone = phone.package;

  # The D80's firmware binary: static, ARMv7 hard-float, with the touch
  # skin.
  opendeskphone-d80 = phoneD80.package;

  # `/init` of the kernel's initramfs: static, ARMv6 soft-float, a few
  # kilobytes.
  opendeskphone-early = early.package;

  # `/init` of the D80's recovery image: static, ARMv7 hard-float.
  opendeskphone-recovery = recovery.package;

  # Host tools.  `opendeskphone-image`: EFF, OTA, kernel image, and JFFS2.
  # `opendeskphone-apps-build`: componentizes and precompiles the WebAssembly
  # apps.
  opendeskphone-image = imageTool.package;
  opendeskphone-apps-build = appsTool.package;

  # The DPMA module, `lib/libres_open_dpma.so`, for the Asterisk of
  # asterisk.nix.
  res_open_dpma = module.package;

  # Unit and integration tests of every crate, on the build machine.
  tests = craneLib.cargoTest (
    checksStage.full
    // {
      cargoArtifacts = checksStage.deps;
      # The check is that this builds; nothing builds on its target
      # directory, so it is not archived into the output.
      doInstallCargoArtifacts = false;
      doCheck = true;
      # Every failure in one run, not only the first crate's; and no test
      # passes by skipping itself for want of an input or a facility of
      # the host (the `test-inputs` crate).  The tests that run in a network
      # namespace are not built (the feature `netns-tests`): they need
      # unprivileged user namespaces, which are not available in CI.
      cargoTestExtraArgs = "--no-fail-fast --features test-inputs/required";
      # The stock firmware, Asterisk with res_open_dpma, and the staged
      # apps, where the tests look for them.
      preCheck = "ln -s ${testInputs} test-inputs";
      # The OpenSSH client, for the test that logs in to the embedded SSH
      # server and pushes a file with `scp`; that test skips itself on a
      # machine without one.
      # qemu-arm, for the tests that run the stock `configmerge`.
      nativeBuildInputs = checksStage.full.nativeBuildInputs ++ [
        openssh
        qemu-user
      ];
    }
    // lib.optionalAttrs (appsUnitTests != null) {
      # `apps/` is a cargo workspace of its own, which `cargo test` here
      # does not reach: its tests run in a derivation of their own, and
      # this check does not pass unless that one did.
      inherit appsUnitTests;
    }
  );

  clippy = craneLib.cargoClippy (
    checksStage.full
    // {
      cargoArtifacts = checksStage.deps;
      doInstallCargoArtifacts = false;
      # The tests that run in a network namespace are linted too, though
      # the test check does not build them.
      cargoClippyExtraArgs = "--all-targets --features opendeskphone/netns-tests,opendeskphone-net/netns-tests -- -D warnings";
    }
    // lib.optionalAttrs (appsClippy != null) {
      # Likewise the apps' workspace, linted in a derivation of its own.
      inherit appsClippy;
    }
  );

  # Dependency layers, exported so they can be kept alive across garbage
  # collection.
  deps = {
    phone = phone.deps;
    phone-d80 = phoneD80.deps;
    recovery = recovery.deps;
    image-tool = imageTool.deps;
    apps-tool = appsTool.deps;
    module = module.deps;
    checks = checksStage.deps;
  };
}
