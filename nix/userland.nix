# The shell and the utilities: brush, uutils coreutils, and uutils sed, all
# Rust, all static binaries with no C in them (spike S7), for `target`:
# ARMv6 for the D40 thru D70, and ARMv7 for the D80 (rust.nix).
#
# Not here, because their uutils implementations cannot be built without a C
# cross compiler, which userspace does not have: `grep` and `find` (uu_grep
# 0.2.0 and findutils 0.11.0 need the Oniguruma C library) and `tar`
# (uutils/tar needs the zstd C library).
{
  lib,
  fetchCrate,
  rustPlatform,
  rust,
  target ? rust.armTarget,
  # The applets of the coreutils binary, or null for the list below.  The
  # D80's recovery has a short list of its own: its ramdisk shares 8 MiB
  # with a kernel.
  only ? null,
}:
let
  # Applets in the multi-call coreutils binary.  Chosen for a phone: file
  # handling, text inspection, checksums for update debugging, and the few
  # system queries that have no `dphone` subcommand.  Mounting, networking,
  # and module loading are `dphone` subcommands, not coreutils.
  applets = if only != null then only else all;
  all = [
    "base64"
    "basename"
    "cat"
    "chmod"
    "chown"
    "cksum"
    "cp"
    "cut"
    "date"
    "dd"
    "df"
    "dirname"
    "du"
    "echo"
    "env"
    "expr"
    "false"
    "head"
    "hostname"
    "id"
    "ln"
    "ls"
    "md5sum"
    "mkdir"
    "mknod"
    "mktemp"
    "mv"
    "od"
    "printenv"
    "printf"
    "pwd"
    "readlink"
    "realpath"
    "rm"
    "rmdir"
    "seq"
    "sha256sum"
    "sleep"
    "sort"
    "stat"
    "stty"
    "sync"
    "tail"
    "tee"
    "test"
    "touch"
    "tr"
    "true"
    "truncate"
    "uname"
    "uniq"
    "uptime"
    "wc"
    "yes"
  ];

  crate =
    {
      pname,
      version,
      srcHash,
      vendorHash,
      cargoArgs ? [ ],
      bin,
    }:
    let
      src = fetchCrate {
        inherit pname version;
        hash = srcHash;
      };
    in
    rust.buildCargo {
      inherit pname version src;
      cargoDeps = rustPlatform.fetchCargoVendor {
        inherit pname version src;
        hash = vendorHash;
      };
      inherit target;
      env = rust.sizeProfileEnv;
      commands = [
        (
          [
            "build"
            "--release"
          ]
          ++ cargoArgs
        )
      ];
      bins = [ bin ];
      meta.license = lib.licenses.mit;
    };
in
{
  inherit applets;

  brush = crate {
    pname = "brush-shell";
    version = "0.4.0";
    srcHash = "sha256-XkwEa86Rn7WACUxD+BrgoiEG/CBjYyh2giTAhqyJOsU=";
    vendorHash = "sha256-7VtzTLqnPp7zdXEcCZRTqDyUh3iXhiATb/uAadF6tx0=";
    bin = "brush";
  };

  coreutils = crate {
    pname = "coreutils";
    version = "0.12.0";
    srcHash = "sha256-PLeCAp+Lat2LhL66B9FM9j7/pf77Qheb9wFB9LIWmyw=";
    vendorHash = "sha256-LROC0h9frPGNPeIIAC5Y4iy34Igb7lTVw2jcT79zuzo=";
    cargoArgs = [
      "--no-default-features"
      "--features"
      (lib.concatStringsSep "," applets)
    ];
    bin = "coreutils";
  };

  sed = crate {
    pname = "sed";
    version = "0.3.0";
    srcHash = "sha256-vRyaJAxCRj0mLcJmJ+14vfqc5N3Z/3TiaHwL7Sd9F98=";
    vendorHash = "sha256-gcFMYF8effLwMPLVU78f419UFtJz2KGv2gVZw8kOdPI=";
    bin = "sed";
  };
}
