# The vendor's stock firmware images.  They are proprietary, so they are
# never in the repository; Nix downloads each from the vendor when
# something needs it, and checks it against the hash here.  The whole
# repository uses exactly two: the D40's for the old family and the D60's
# for the new one, and a third, the D80's (below), only for an image for
# the D80.  Two things need them:
#
# - An image build needs the voice DSP firmware, which is a file in the
#   stock root file system (dsp-firmware.nix).
# - The checks compare against them: the EFF/OTA round trips, the JFFS2
#   structure checks, and the stock `configmerge` (run under qemu-arm) that
#   our configuration merge must agree with.
#
# The downloads are fixed-output derivations: Nix looks for the result by
# its hash before it fetches.  A store that already holds an image builds
# without the network, whether or not the vendor still serves it.  To put a
# copy you have into the store by hand:
#
#   nix store add-file 2_9_28_D60_firmware.eff
{
  lib,
  fetchurl,
  runCommand,
  opendeskphone-image, # the host tool (not the `opendeskphone` package: see dsp-firmware.nix)
}:
let
  # The releases the tests' expectations were taken from.
  images = {
    D40 = {
      version = "2_9_26";
      sha256 = "7b038b4fb03026cdb49bff3ec3809804912be6f6a591abd1d4ebc6a0e9ac9edd";
    };
    D60 = {
      version = "2_9_28";
      sha256 = "2e55006abb7e6e51979c2556865900b4d38afffd504fa8aaa360b08f59f6dd02";
    };
  };

  eff =
    model:
    { version, sha256 }:
    let
      name = "${version}_${model}_firmware.eff";
    in
    fetchurl {
      inherit name sha256;
      url = "https://phones.dl.sangoma.com/firmware/asterisk/${version}/${name}";
    };
  effs = lib.mapAttrs eff images;

  # The D80's stock image, for what an image for the D80 takes from it
  # (d80-firmware.nix).  Inside it is an Android OTA package, not a kernel
  # and a root file system image, so it is not among `images`, and the tests
  # do not read it.
  d80 = eff "D80" {
    version = "1_12_14";
    sha256 = "d64d386d4a9733e92d7aff37eb9251e857195c2e4352c972111c205144cbbbc2";
  };

  # Each image taken apart with our own tool: DIR/kernel.bin and
  # DIR/rootfs.jffs2.
  parts = lib.mapAttrs (
    model: file:
    runCommand "stock-${lib.toLower model}-parts" { nativeBuildInputs = [ opendeskphone-image ]; } ''
      opendeskphone-image eff unpack ${file} firmware.ota
      opendeskphone-image ota unpack firmware.ota $out
    ''
  ) effs;
in
{
  inherit effs parts d80;

  # The files the tests read (test-inputs/vendor): one directory per
  # model, with the image and its root file system image.
  dir = runCommand "stock-firmware" { } (
    lib.concatStrings (
      lib.mapAttrsToList (model: file: ''
        mkdir -p $out/${model}
        ln -s ${file} $out/${model}/${file.name}
        ln -s ${parts.${model}}/rootfs.jffs2 $out/${model}/rootfs.jffs2
      '') effs
    )
  );

  # The stock D60 root file system as a directory (test-inputs/stock-rootfs),
  # for running its programs under qemu-arm.  Device nodes are left out.
  rootfs = runCommand "stock-d60-rootfs" { nativeBuildInputs = [ opendeskphone-image ]; } ''
    opendeskphone-image jffs2 extract ${parts.D60}/rootfs.jffs2 $out > extract.log
    test -x $out/app/bin/configmerge
  '';
}
