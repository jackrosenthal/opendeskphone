# Linux v7.3.5 with the ports to the phones: kernel/patches applied in
# name order, kernel/files copied over the tree, configured from
# kernel/configs.  The one source tree has both ports, and this file builds
# one kernel of it; nix/default.nix calls it once for each:
#
#   bcmring (the default arguments)   the D40, D45, D50, D60, D62, D65, and
#                                     D70.  One build serves all seven; only
#                                     the appended DTB differs.
#   d80_defconfig                     the D80 (i.MX 6DualLite)
#
# With `earlyInit`, the kernel image carries an initramfs with one program,
# the early-boot stage (crates/opendeskphone-early): it counts trial boots and
# can select the previous bank before the root file system is needed, then
# mounts the root and runs its init.  The initramfs is not compressed by
# itself; it is compressed with the rest of the zImage.
#
# Output layout:
#   zImage                       XZ-compressed kernel, no DTB appended
#   dtbs/<board>.dtb             the board devicetrees
#   zImage-dtb                   with `appendDtb`: zImage and then that
#                                board's DTB, which is how the D80's boot
#                                loader takes the two
#   lib/modules/<release>/...    stripped modules.  With `depmod`, also
#                                `modules.dep`, from which the init role
#                                takes what each module needs loaded before
#                                it; without, no indexes (the init role's
#                                load list for the D40 thru D70 spells out
#                                the order)
#   config                       the resolved .config
{
  lib,
  stdenv,
  fetchurl,
  buildPackages,
  pkgsCross,
  bc,
  bison,
  flex,
  perl,
  xz,
  openssl,
  kmod,
  kernelSrc, # path: the repository's kernel/ directory
  pname ? "linux-bcmring",
  defconfig ? "bcmring_tiny_defconfig",
  # The built opendeskphone-early ($out/bin/opendeskphone-early, static ARM)
  # and crates/opendeskphone-early/initramfs.list, or null for a kernel
  # without the early-boot stage.
  earlyInit ? null,
  initramfsList ? null,
  # The make targets that build the devicetrees, and which of the built
  # ones to install, as a glob under arch/arm/boot/dts.
  dtbTargets ? [ "dtbs" ],
  dtbs ? "broadcom/*-digium-*.dtb",
  # Model to devicetree name.
  boards ? {
    d40 = "bcm11170-digium-d40";
    d45 = "bcm11109-digium-d45";
    d50 = "bcm11170-digium-d50";
    d60 = "bcm11189-digium-d60";
    d62 = "bcm11188-digium-d62";
    d65 = "bcm11188-digium-d65";
    d70 = "bcm11109-digium-d70";
  },
  # A devicetree name: also install zImage-dtb, the zImage with that
  # devicetree after it.
  appendDtb ? null,
  # Write `modules.dep` for the installed modules.
  depmod ? false,
}:
let
  version = "7.3.5";
  cross = pkgsCross.arm-embedded.buildPackages;
  patchDir = kernelSrc + "/patches";
  patches = map (n: patchDir + "/${n}") (
    lib.sort lib.lessThan (
      lib.attrNames (
        lib.filterAttrs (n: t: t == "regular" && lib.hasSuffix ".patch" n) (builtins.readDir patchDir)
      )
    )
  );
  makeFlags = [
    "ARCH=arm"
    "CROSS_COMPILE=arm-none-eabi-"
    "O=build"
    "HOSTCC=cc"
    "KBUILD_BUILD_USER=opendeskphone"
    "KBUILD_BUILD_HOST=opendeskphone"
    # No local version, so the module directory is named by the version alone.
    "LOCALVERSION="
    "KBUILD_BUILD_TIMESTAMP=1970-01-01"
  ];
  src = fetchurl {
    url = "mirror://kernel/linux/kernel/v7.x/linux-${version}.tar.xz";
    hash = "sha256-jyk/g9VPC3ub5Whj68Xk6nnXPlMLSuVcVyvpfvOfCZA=";
  };

  applyPort = ''
    cp -r --no-preserve=mode ${kernelSrc + "/files"}/. .
    cp ${kernelSrc + "/configs"}/* arch/arm/configs/
  '';

  # The v7.3.5 tree with the ports applied and nothing built: what a developer
  # copies over a git checkout of v7.3.5 to work on them (kernel/README.md).
  source = stdenv.mkDerivation {
    pname = "linux-opendeskphone-source";
    inherit version src patches;
    postPatch = applyPort;
    dontConfigure = true;
    dontBuild = true;
    dontFixup = true;
    installPhase = "cp -r . $out";
  };
in
stdenv.mkDerivation {
  inherit
    pname
    version
    patches
    makeFlags
    ;

  inherit src;

  depsBuildBuild = [ buildPackages.stdenv.cc ];
  nativeBuildInputs = [
    cross.gcc
    cross.binutils
    bc
    bison
    flex
    perl
    xz
    openssl
  ]
  ++ lib.optional depmod kmod;

  # The compiler wrapper's hardening flags are for hosted userspace.
  hardeningDisable = [ "all" ];
  enableParallelBuilding = true;

  postPatch = applyPort + ''
    patchShebangs scripts
  '';

  configurePhase = ''
    runHook preConfigure
    make $makeFlags ${defconfig}
  ''
  + lib.optionalString (earlyInit != null) ''
    sed 's|@INIT@|${earlyInit}/bin/opendeskphone-early|' ${initramfsList} > initramfs.list
    scripts/config --file build/.config --set-str INITRAMFS_SOURCE "$PWD/initramfs.list"
    make $makeFlags olddefconfig
    grep -q '^CONFIG_INITRAMFS_COMPRESSION_NONE=y' build/.config
  ''
  + ''
    runHook postConfigure
  '';

  buildPhase = ''
    runHook preBuild
    make $makeFlags -j$NIX_BUILD_CORES zImage ${lib.escapeShellArgs dtbTargets} modules
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p $out/dtbs
    cp build/arch/arm/boot/zImage $out/zImage
    cp build/arch/arm/boot/dts/${dtbs} $out/dtbs/
    ${lib.optionalString (
      appendDtb != null
    ) "cat $out/zImage $out/dtbs/${appendDtb}.dtb > $out/zImage-dtb"}
    cp build/.config $out/config
    make $makeFlags INSTALL_MOD_PATH=$out INSTALL_MOD_STRIP=1 DEPMOD=true modules_install
    rm -f $out/lib/modules/*/build $out/lib/modules/*/source
    ${lib.optionalString depmod ''
      depmod --basedir $out ${version}
      # Only the text index is read; the others are for a `modprobe` that
      # the root file system does not have.
      find $out/lib/modules/${version} -maxdepth 1 -name 'modules.*' \
        ! -name modules.dep ! -name modules.order ! -name modules.builtin -delete
      test -s $out/lib/modules/${version}/modules.dep
    ''}
    runHook postInstall
  '';

  dontStrip = true;
  dontPatchELF = true;

  passthru.source = source;
  passthru.boards = boards;

  meta.license = lib.licenses.gpl2Only;
}
