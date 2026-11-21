# Everything the flake exports.  Each artifact is its own small derivation:
#
#   kernel                 zImage, DTBs, modules (one build for all models)
#   kernel-source          v7.3 with the port applied, unbuilt (for editing)
#   kernel-<model>         boot image for the k0/k1 slot
#   opendeskphone          the firmware binary of the phones with keys, D40
#                          to D70 (ARMv6, static)
#   opendeskphone-d80      the firmware binary with the D80's touch skin (no
#                          image is made of it yet)
#   opendeskphone-image    host tool: EFF, OTA, kernel image, rootfs
#   brush, coreutils, sed  shell and utilities (ARMv6, static)
#   apps                   built-in phone apps (.wasm)
#   asterisk-modules       Asterisk with res_open_dpma, for the tests (x86-64)
#   web                    frontend bundle
#   rootfs-tree            the root file system as a directory
#   rootfs                 the JFFS2 image
#   eff-<model>            flashable image, one per model
#   all                    the seven EFFs in one directory
{
  pkgs,
  lib,
  crane,
}:
let
  root = ../.;
  has = p: builtins.pathExists (root + "/${p}");

  rustToolchain = pkgs.rust-bin.stable.latest.minimal.override {
    extensions = [
      "clippy"
      "rustfmt"
      "rust-src"
    ];
    targets = [
      "arm-unknown-linux-musleabi"
      "wasm32-wasip2"
      "wasm32-unknown-unknown"
    ];
  };
  rustPlatform = pkgs.makeRustPlatform {
    cargo = rustToolchain;
    rustc = rustToolchain;
  };
  rust = pkgs.callPackage ./rust.nix { inherit rustToolchain rustPlatform; };
  craneLib = (crane.mkLib pkgs).overrideToolchain (_: rustToolchain);

  userland = pkgs.callPackage ./userland.nix { inherit rustPlatform rust; };
  kernel = pkgs.callPackage ./kernel.nix {
    kernelSrc = root + "/kernel";
    earlyInit = workspace.opendeskphone-early;
    initramfsList = root + "/crates/opendeskphone-early/initramfs.list";
  };
  # The vendor's stock images, downloaded when something needs one, and the
  # DSP firmware taken out of two of them.
  stock = pkgs.callPackage ./vendor-firmware.nix { inherit (workspace) opendeskphone-image; };
  dspFirmware = pkgs.callPackage ./dsp-firmware.nix {
    inherit stock;
    inherit (workspace) opendeskphone-image;
  };

  # The hash of the fetched pnpm store; when web/pnpm-lock.yaml changes, set
  # it to lib.fakeHash, build, and copy the hash Nix reports.
  web = pkgs.callPackage ./web.nix { pnpmDepsHash = "sha256-eKjJ675HiFF5dqpBcEQtN8emaPRtxpHTf2mMOXHcwFg="; };

  asterisk = pkgs.callPackage ./asterisk.nix { module = workspace.res_open_dpma; };
  workspace = pkgs.callPackage ./workspace.nix {
    inherit craneLib rust rustPlatform;
    inherit asterisk testInputs;
    webDist = web;
    appsUnitTests = if apps != null then apps.unitTests else null;
    appsClippy = if apps != null then apps.clippy else null;
  };
  # What the tests need that is not in the repository, as they find it in
  # `test-inputs/` (crates/test-inputs): the `tests` check links it there,
  # and `nix build .#test-inputs -o test-inputs` does in a checkout.
  testInputs = pkgs.linkFarm "test-inputs" (
    [
      {
        name = "vendor";
        path = stock.dir;
      }
      {
        name = "stock-rootfs";
        path = stock.rootfs;
      }
      {
        name = "asterisk";
        path = asterisk.server;
      }
      {
        name = "asterisk-modules";
        path = asterisk.modules;
      }
    ]
    ++ lib.optional (apps != null) {
      name = "apps-staged";
      path = apps.testFixture;
    }
  );

  apps =
    if has "apps/Cargo.lock" then
      pkgs.callPackage ./apps.nix {
        inherit rustPlatform rust;
        inherit (workspace) opendeskphone-apps-build;
      }
    else
      null;

  image = pkgs.callPackage ./image.nix {
    inherit (workspace) opendeskphone opendeskphone-image version;
    inherit (userland)
      brush
      coreutils
      applets
      sed
      ;
    inherit kernel dspFirmware apps;
  };

  effs = lib.mapAttrs' (m: v: lib.nameValuePair "eff-${m}" v) image.effs;
  kernels = lib.mapAttrs' (m: v: lib.nameValuePair "kernel-${m}" v) image.kernelImages;
in
{
  packages = {
    inherit kernel;
    kernel-source = kernel.source;
    inherit (userland) brush coreutils sed;
    inherit (workspace)
      opendeskphone
      opendeskphone-d80
      opendeskphone-early
      opendeskphone-image
      ;
    asterisk-modules = asterisk.modules;
    rootfs-tree = image.rootfsTree;
    inherit (image) rootfs;
    all = pkgs.linkFarm "opendeskphone-${workspace.version}" (
      lib.mapAttrsToList (m: v: {
        name = v.name;
        path = v;
      }) image.effs
    );
    default = image.effs.d60;
    inherit web;
    test-inputs = testInputs;
  }
  // kernels
  // effs
  // lib.optionalAttrs (apps != null) { inherit apps; };

  # `tests` compares against the D40 and D60 stock firmware images
  # (vendor-firmware.nix), and runs the main role against Asterisk with
  # res_open_dpma (asterisk.nix).
  checks = {
    inherit (workspace) tests clippy;
    inherit kernel;
    inherit (userland) brush coreutils sed;
    # Each of the two firmware binaries is checked for the touch skin as it
    # is built: the one must not have it, and the other must
    # (workspace.nix).
    inherit (workspace) opendeskphone opendeskphone-d80;
    # The rootfs image under the kernel's JFFS2 driver, on simulated NAND.
    jffs2-mount = pkgs.callPackage ./jffs2-mount-test.nix {
      image = image.rootfs;
      tree = image.rootfsTree;
    };
  };

  devShell = pkgs.mkShell {
    packages = [
      rustToolchain
      pkgs.protobuf
      pkgs.buf
      pkgs.nodejs
      pkgs.pnpm
      pkgs.qemu
      pkgs.pkgsCross.arm-embedded.buildPackages.gcc
      pkgs.pkgsCross.arm-embedded.buildPackages.binutils
      pkgs.bc
      pkgs.bison
      pkgs.flex
      pkgs.perl
      pkgs.kmod
      pkgs.dtc
      pkgs.nixfmt
    ];
  };
}
