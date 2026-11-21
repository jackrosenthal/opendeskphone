# Rust toolchain helpers.  Userspace for the phone is built for
# arm-unknown-linux-musleabi (ARMv6, soft-float, static) and linked with the
# toolchain's own rust-lld and self-contained musl objects, so no C cross
# compiler is involved.  The D80's userspace is built the same way for
# armv7-unknown-linux-musleabihf (its Cortex-A9, with its floating point
# unit).  The stdenv's native C compiler is only used to link
# build scripts and proc macros, which run on the build machine.
{
  lib,
  stdenv,
  rustToolchain,
  rustPlatform,
  removeReferencesTo,
}:
rec {
  armTarget = "arm-unknown-linux-musleabi";

  armEnv = {
    CARGO_BUILD_TARGET = armTarget;
    CARGO_TARGET_ARM_UNKNOWN_LINUX_MUSLEABI_LINKER = "rust-lld";
    CARGO_TARGET_ARM_UNKNOWN_LINUX_MUSLEABI_RUSTFLAGS = lib.concatStringsSep " " [
      "-C target-cpu=arm1176jz-s"
      "-C target-feature=+crt-static"
      "-C link-self-contained=yes"
    ];
  };

  # The D80: an i.MX 6DualLite, two Cortex-A9 cores with VFPv3 and NEON.
  armv7Target = "armv7-unknown-linux-musleabihf";

  armv7Env = {
    CARGO_BUILD_TARGET = armv7Target;
    CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER = "rust-lld";
    CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_RUSTFLAGS = lib.concatStringsSep " " [
      "-C target-cpu=cortex-a9"
      "-C target-feature=+crt-static"
      "-C link-self-contained=yes"
    ];
  };

  # Size-first release profile, for programs whose own Cargo.toml we do not
  # control (brush, uutils).  The workspace sets its own profile.
  sizeProfileEnv = {
    CARGO_PROFILE_RELEASE_OPT_LEVEL = "z";
    CARGO_PROFILE_RELEASE_LTO = "fat";
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "1";
    CARGO_PROFILE_RELEASE_PANIC = "abort";
    CARGO_PROFILE_RELEASE_STRIP = "true";
  };

  # Run cargo in a project with vendored dependencies.
  #   target:    null for the build machine, or a rustc target triple; picks
  #              the directory binaries are installed from.
  #   commands:  cargo invocations, each a list of arguments after `cargo`.
  #              Every one runs offline against the locked dependency set.
  #   bins:      executables to install into $out/bin (may be empty).
  #   artifacts: other files to install into $out: shell globs relative to
  #              the release output directory (e.g., "*.wasm").
  buildCargo =
    {
      pname,
      version,
      src,
      cargoDeps,
      commands,
      bins ? [ ],
      artifacts ? [ ],
      target ? null,
      env ? { },
      nativeBuildInputs ? [ ],
      preBuild ? "",
      cargoRoot ? null,
      meta ? { },
    }:
    let
      outDir = "target/${lib.optionalString (target != null) "${target}/"}release";
      cross = target != null;
    in
    stdenv.mkDerivation (
      {
        inherit
          pname
          version
          src
          cargoDeps
          preBuild
          meta
          ;
        nativeBuildInputs = [
          rustToolchain
          rustPlatform.cargoSetupHook
          removeReferencesTo
        ]
        ++ nativeBuildInputs;
        env =
          (lib.optionalAttrs (target == armTarget) armEnv)
          // (lib.optionalAttrs (target == armv7Target) armv7Env)
          // env;
        # The hardening wrapper flags are for the native C compiler; they
        # must not leak into the ARM link, which rust-lld does by itself.
        hardeningDisable = [ "all" ];
        dontStrip = cross;
        dontPatchELF = cross;
        buildPhase = ''
          runHook preBuild
          ${lib.optionalString (cargoRoot != null) "cd ${cargoRoot}"}
          ${lib.concatMapStringsSep "\n" (
            c: "cargo ${lib.head c} --offline --locked -j $NIX_BUILD_CORES ${lib.escapeShellArgs (lib.tail c)}"
          ) commands}
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          mkdir -p $out
          ${lib.concatMapStringsSep "\n" (b: "install -D -m 0755 ${outDir}/${b} $out/bin/${b}") bins}
          ${lib.concatMapStringsSep "\n" (a: "install -m 0644 ${outDir}/${a} $out/") artifacts}
          # Panic messages from the standard library's generic code name its
          # source files under the toolchain's store path, which would make
          # the output, and the image it goes into, depend on the toolchain.
          # The hash in those paths is overwritten, as crane does for the
          # workspace's own binaries; nothing changes size.
          find $out -type f -exec remove-references-to -t ${rustToolchain} {} +
          runHook postInstall
        '';
      }
      // lib.optionalAttrs (cargoRoot != null) { inherit cargoRoot; }
    );
}
