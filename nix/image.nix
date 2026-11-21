# Boot images, the root file system, and the per-model EFFs.  Every step is one
# call of the `opendeskphone-image` tool; the root file system's contents are
# listed here and laid out with plain installs.
#
# The D40 thru D70 share one root file system, a JFFS2 image for their NAND
# flash, and an EFF with an OTA container of the kernel and that image.  The
# D80 has its own (`d80` below): a SquashFS image for its eMMC, an Android
# boot image, and an Android OTA package of the two, which is also what its
# EFF carries.
{
  lib,
  runCommand,
  squashfsTools,
  opendeskphone-image,
  kernel,
  kernelD80,
  opendeskphone,
  opendeskphone-d80,
  d80Firmware,
  userlandD80, # brush, coreutils, and sed built for the D80
  brush,
  coreutils,
  applets,
  sed,
  dspFirmware,
  cacert,
  apps ? null,
  version, # e.g., "3.1.0"
}:
let
  # Stock-style version string: 3_1_0.
  wireVersion = lib.replaceStrings [ "." ] [ "_" ] version;

  models = {
    d40 = {
      name = "D40";
      family = "old";
    };
    d45 = {
      name = "D45";
      family = "old";
    };
    d50 = {
      name = "D50";
      family = "old";
    };
    d70 = {
      name = "D70";
      family = "old";
    };
    d60 = {
      name = "D60";
      family = "new";
    };
    d62 = {
      name = "D62";
      family = "new";
    };
    d65 = {
      name = "D65";
      family = "new";
    };
  };

  # zImage + the model's DTB, padded, header-patched for boot2 (load address
  # 0x02000000).  The tool fails when the result does not fit the 2 MiB
  # k0/k1 slot.
  kernelImage =
    model: info:
    runCommand "kernel-${model}-${version}" { nativeBuildInputs = [ opendeskphone-image ]; } ''
      mkdir -p $out
      opendeskphone-image kernel-image \
        --model ${model} \
        --zimage ${kernel}/zImage \
        --dtb ${kernel}/dtbs/${kernel.boards.${model}}.dtb \
        --load 0x02000000 \
        --out $out/kernel.bin
    '';

  # A root file system tree: the firmware binary, the shell and the
  # utilities, a kernel's modules, the apps, and the sounds.  `firmware`
  # is the commands that put the family's firmware files under
  # $out/lib/firmware.
  tree =
    {
      name,
      dphone,
      kernel,
      firmware,
      brush,
      coreutils,
      sed,
      # The ELF header's flags word of every program: the ABI version, and
      # whether floating point values are passed in the floating point
      # unit's registers.
      elfFlags,
    }:
    runCommand name { } ''
    mkdir -p $out/{bin,sbin,etc/opendeskphone,lib/firmware,usr/share/opendeskphone/apps}
    mkdir -p $out/{dev,proc,sys,tmp,run,var,nvdata,root}

    install -m 0755 ${dphone}/bin/dphone $out/sbin/init
    ln -s ../sbin/init $out/bin/dphone
    # System commands `dphone` provides under its own name.
    for c in mount umount insmod rmmod lsmod modprobe reboot poweroff halt dmesg ps; do
      ln -s ../sbin/init $out/bin/$c
    done

    install -m 0755 ${brush}/bin/brush $out/bin/brush
    ln -s brush $out/bin/sh
    install -m 0755 ${coreutils}/bin/coreutils $out/bin/coreutils
    for a in ${lib.escapeShellArgs applets}; do
      ln -s coreutils $out/bin/$a
    done
    ln -s test "$out/bin/["
    install -m 0755 ${sed}/bin/sed $out/bin/sed

    cp -r ${kernel}/lib/modules $out/lib/modules

    ${firmware}

    ${lib.optionalString (
      apps != null
    ) "cp -r ${apps}/. $out/usr/share/opendeskphone/apps/ && chmod -R u+w $out/usr/share/opendeskphone/apps"}

    # The alarm clock's tones, with their notice and license, which must go
    # wherever the tones go (assets/alarm-tones/README.md).
    install -d $out/usr/share/opendeskphone/alarm-tones
    install -m 0644 ${../assets/alarm-tones}/*.wav ${../assets/alarm-tones}/NOTICE \
      ${../assets/alarm-tones}/LICENSE-Apache-2.0.txt $out/usr/share/opendeskphone/alarm-tones/

    # The built-in ringtones, likewise (assets/ringtones/README.md).
    install -d $out/usr/share/opendeskphone/ringtones
    install -m 0644 ${../assets/ringtones}/*.wav ${../assets/ringtones}/NOTICE \
      ${../assets/ringtones}/LICENSE-Apache-2.0.txt $out/usr/share/opendeskphone/ringtones/

    # The base trust bundle (Mozilla's certificate authorities for TLS
    # servers), at the path stock keeps its own.  `dphone` builds
    # `/nvdata/certs/certs.crt` from it and the provisioned certificates.
    # This is nixpkgs' plain PEM bundle, the kind stock ships;
    # `ca-bundle.crt` repeats each certificate as an OpenSSL
    # `TRUSTED CERTIFICATE` block, which rustls does not read, and adds the
    # authorities for e-mail only.
    install -D -m 0644 ${cacert}/etc/ssl/certs/ca-no-trust-rules-bundle.crt $out/usr/share/ssl/certs/cacert.pem
    grep -q -e '-----BEGIN CERTIFICATE-----' $out/usr/share/ssl/certs/cacert.pem
    ! grep -q -e '-----BEGIN TRUSTED CERTIFICATE-----' $out/usr/share/ssl/certs/cacert.pem

    printf 'root:x:0:0:root:/nvdata/root:/bin/brush\n' > $out/etc/passwd
    printf 'root:x:0:\ntty:x:5:\n' > $out/etc/group
    printf '/bin/brush\n/bin/sh\n' > $out/etc/shells
    # The interactive shells' defaults: the prompt, aliases, and the tab
    # completion of `dphone`.
    install -m 0644 ${./rootfs/etc/bash.bashrc} $out/etc/bash.bashrc
    # The operating system's name and version, as os-release(5) has them:
    # the file under /usr/lib, and /etc/os-release a relative link to it.
    mkdir -p $out/usr/lib
    printf '%s\n' \
      'NAME="Open Desk Phone"' \
      'ID=opendeskphone' \
      'VERSION_ID=${version}' \
      'PRETTY_NAME="Open Desk Phone ${version}"' \
      > $out/usr/lib/os-release
    ln -s ../usr/lib/os-release $out/etc/os-release

    # What the kernel starts must be there and must be a program for the
    # phone: a root without it boots to "No working init found", and only
    # a phone would say so.  /sbin/init, and the shell the kernel falls
    # back to, each a nonempty executable ELF file: 32-bit, little-endian,
    # for ARM (machine 40).
    for program in sbin/init bin/sh; do
      file=$(realpath -e $out/$program)
      case $file in $out/*) ;; *) echo "$program leads out of the root: $file" >&2; exit 1 ;; esac
      test -s "$file" -a -x "$file"
      test "$(od -An -tx1 -N6 "$file" | tr -d ' \n')" = 7f454c460101
      test "$(od -An -tu2 -j18 -N2 "$file" | tr -d ' \n')" = 40
    done
    # Every program is built for this family's processor: an image must
    # not mix the two.
    for program in sbin/init bin/brush bin/coreutils bin/sed; do
      test "$(od -An -tx4 -j36 -N4 $out/$program | tr -d ' \n')" = ${elfFlags}
    done
  '';

  # The root file system tree of the D40 thru D70.  It is the same for
  # every one of them: both DSP firmware variants are present, each in its
  # family's directory, and the init role points the kernel's firmware
  # loader at the right one.
  rootfsTree = tree {
    name = "opendeskphone-rootfs-tree-${version}";
    dphone = opendeskphone;
    inherit
      kernel
      brush
      coreutils
      sed
      ;
    # EABI version 5, soft-float.
    elfFlags = "05000200";
    firmware = ''
      install -D -m 0644 ${dspFirmware.old} $out/lib/firmware/old-family/dspapp1107_lcc.bin
      install -D -m 0644 ${dspFirmware.new} $out/lib/firmware/new-family/dspapp1107_lcc.bin
    '';
  };

  # JFFS2 image of the tree: little-endian, 128 KiB erase blocks, all files
  # owned by root.
  rootfs = runCommand "opendeskphone-rootfs-${version}.jffs2" { nativeBuildInputs = [ opendeskphone-image ]; } ''
    opendeskphone-image jffs2 build ${rootfsTree} $out \
      --mtime 0 \
      --node dev/console:c:5:1:600 \
      --node dev/null:c:1:3:666
  '';

  eff =
    model: info:
    runCommand "${wireVersion}_${info.name}_firmware.eff" { nativeBuildInputs = [ opendeskphone-image ]; } ''
      opendeskphone-image image build \
        --model ${info.name} \
        --version ${wireVersion} \
        --kernel ${kernelImage model info}/kernel.bin \
        --rootfs-image ${rootfs} \
        --mtime 0 \
        --out $out
    '';

  # The D80.  Its eMMC keeps the stock partitions (the table is in
  # crates/opendeskphone-image/src/d80.rs): the boot image goes to the
  # first, the root file system to the stock `/system`, and nvdata is the
  # stock `/cache`.  The stock recovery and `/data` are left as they are.
  d80 = rec {
    # The firmware binary, the shell, and the utilities built for the
    # D80, its kernel's modules, and the firmware files its kernel loads,
    # under the names the mainline drivers ask for (kernel/README.md).
    rootfsTree = tree {
      name = "opendeskphone-d80-rootfs-tree-${version}";
      dphone = opendeskphone-d80;
      kernel = kernelD80;
      inherit (userlandD80) brush coreutils sed;
      # EABI version 5, hard-float.
      elfFlags = "05000400";
      firmware = ''
        install -D -m 0644 ${d80Firmware.zl38040} $out/lib/firmware/zl38040.fw
        install -D -m 0644 ${d80Firmware.wifi} $out/lib/firmware/brcm/brcmfmac43430a0-sdio.bin
        install -D -m 0644 ${d80Firmware.wifiNvram} $out/lib/firmware/brcm/brcmfmac43430a0-sdio.txt
        install -D -m 0644 ${d80Firmware.bluetooth} $out/lib/firmware/brcm/BCM4343A0.hcd
      '';
    };

    # SquashFS image of the tree, as the kernel mounts it read-only from
    # mmcblk0p5 (`root=` in kernel/configs/d80_defconfig): every file owned
    # by root, every time zero.  It needs no device nodes: the kernel
    # mounts devtmpfs on /dev before it starts init.
    rootfs = runCommand "opendeskphone-d80-rootfs-${version}.squashfs" { nativeBuildInputs = [ squashfsTools ]; } ''
      # mksquashfs takes its times from the options, and refuses both.
      unset SOURCE_DATE_EPOCH
      mksquashfs ${rootfsTree} $out -all-root -noappend -no-xattrs -no-progress \
        -comp xz -b 131072 -mkfs-time 0 -all-time 0
    '';

    # The Android boot image for mmcblk0p1: the kernel with its
    # devicetree after it, and an empty ramdisk.  The tool fails when the
    # result does not fit the 8 MiB partition.
    bootImage = runCommand "boot-d80-${version}.img" { nativeBuildInputs = [ opendeskphone-image ]; } ''
      opendeskphone-image d80 boot-image \
        --kernel ${kernelD80}/zImage-dtb \
        --cmdline "$(sed -n 's/^CONFIG_CMDLINE="\(.*\)"$/\1/p' ${kernelD80}/config)" \
        --out $out
    '';

    # The Android OTA package that writes the two: what the stock recovery
    # installs.  It is not signed.
    package = runCommand "${wireVersion}_D80_firmware.zip" { nativeBuildInputs = [ opendeskphone-image ]; } ''
      opendeskphone-image d80 package \
        --boot ${bootImage} \
        --rootfs ${rootfs} \
        --update-binary ${d80Firmware.updateBinary} \
        --version ${wireVersion} \
        --out $out
    '';

    # The package in the vendor's envelope.  A stock image has the vendor's
    # signature after the package, which this has not.
    eff = runCommand "${wireVersion}_D80_firmware.eff" { nativeBuildInputs = [ opendeskphone-image ]; } ''
      opendeskphone-image eff pack ${package} $out --model D80 --version ${wireVersion}
    '';
  };
in
{
  inherit
    models
    rootfsTree
    rootfs
    d80
    ;
  kernelImages = lib.mapAttrs kernelImage models;
  effs = lib.mapAttrs eff models;
}
