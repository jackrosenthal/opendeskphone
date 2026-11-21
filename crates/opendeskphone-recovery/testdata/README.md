# Test data

A made-up boot image, and a patch that makes a made-up recovery image of it, in the forms a stock D80 package has them.  Nothing here is from the vendor.

| File | Contents |
| --- | --- |
| `boot.img` | An Android boot image: a kernel of 768 bytes, and a gzip ramdisk. |
| `recovery-from-boot.p` | An `imgdiff` patch from `boot.img` to a recovery image with the same kernel and the ramdisk `recovery-ramdisk`. |
| `recovery-resource.dat` | The data that the patch takes to follow the boot image's unpacked ramdisk. |
| `recovery-ramdisk` | The recovery image's ramdisk, unpacked. |

The files were made with Python's `zlib` and `bz2`, as Android's `imgdiff` makes a patch with zlib and bzip2.  Each `bsdiff` patch in the file is the plainest one: the bytes that are alike where they are, and the rest as new bytes.
