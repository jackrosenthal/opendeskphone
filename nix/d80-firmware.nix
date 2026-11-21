# What an image for the D80 needs from the vendor's stock firmware.  All of
# it is proprietary, so none of it is in the repository: the build takes
# each file from the stock image, which Nix downloads from the vendor
# (vendor-firmware.nix) and takes apart with our own `opendeskphone-image`.
#
#   zl38040                the firmware of the ZL38040 voice processor.  It
#                          is nowhere but inside the stock kernel, as the
#                          text of S-records; `opendeskphone-image d80
#                          zl38040-firmware` finds it there and writes it in
#                          the form the mainline driver loads
#                          (kernel/README.md).
#   wifi, wifiNvram        the firmware and the board's NVRAM file of the
#                          Wi-Fi half of the AP6212 module
#   bluetooth              the patch file of its Bluetooth half
#   updateBinary           the program a package brings for the stock
#                          recovery to run, which interprets the package's
#                          script.  It is the stock package's own (Android's
#                          `updater`, as the vendor built it).
#
# Each is a fixed-output derivation, so the file is pinned by the hash here,
# and a store that already holds it needs neither the stock image (195 MB)
# nor the network.  The hashes are those of the files in stock release
# 1_12_14.
{
  runCommand,
  unzip,
  opendeskphone-image,
  stock, # vendor-firmware.nix
}:
let
  # The stock image's Android OTA package.  After the zip come 256 bytes
  # (the vendor's signature of the image), which `unzip` passes over.
  fromPackage =
    {
      name,
      sha256,
      path ? null, # a file of the package, or
      script ? null, # commands that make $out from the package's `boot.img`
    }:
    runCommand name
      {
        nativeBuildInputs = [
          opendeskphone-image
          unzip
        ];
        outputHashMode = "flat";
        outputHashAlgo = "sha256";
        outputHash = sha256;
      }
      ''
        opendeskphone-image eff unpack ${stock.d80} update.zip > /dev/null
        ${
          if path != null then
            ''
              unzip -q -o update.zip ${path} || test -s ${path}
              install -m 0644 ${path} $out
            ''
          else
            ''
              unzip -q -o update.zip boot.img || test -s boot.img
              ${script}
            ''
        }
      '';
in
{
  zl38040 = fromPackage {
    name = "zl38040.fw";
    sha256 = "d403cf97e2a717b94da84bc748effbbac7f142dc82c10616e240a5576e33795b";
    script = "opendeskphone-image d80 zl38040-firmware --boot-image boot.img --out $out";
  };
  wifi = fromPackage {
    name = "brcmfmac43430a0-sdio.bin";
    sha256 = "9495a5584db1aa3b31ca8f94e454be92a73d2e75fee030987cbb07c3226e4e0e";
    path = "system/etc/firmware/fw_bcm43438a0.bin";
  };
  wifiNvram = fromPackage {
    name = "brcmfmac43430a0-sdio.txt";
    sha256 = "29dd1ebb2f9691ae10acd1227e243528ebe32b95d6af4831a6f63ab5dafa47cd";
    path = "system/etc/firmware/nvram_ap6212.txt";
  };
  bluetooth = fromPackage {
    name = "BCM4343A0.hcd";
    sha256 = "2b2f1f068709367a4811f86c60cd43b33ad992d1d3622dac6e47fc548c559f2f";
    path = "system/etc/firmware/BCM43438A0.hcd";
  };
  updateBinary = fromPackage {
    name = "update-binary";
    sha256 = "6b513af0def9b823cde5d6c9681c37162c3b9cdfb1be797877f7cd50659a2e37";
    path = "META-INF/com/google/android/update-binary";
  };
}
