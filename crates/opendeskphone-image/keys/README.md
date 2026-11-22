# The D80's OTA package key

`d80-ota.key.pem` is an RSA private key (2048 bits, PKCS #8), and `d80-ota.x509.pem` is its self-signed certificate.  The build uses them to sign the D80's OTA package (`opendeskphone-image d80 package`, `nix build .#ota-d80`), and uses the same key to sign the image that wraps the package (`opendeskphone-image eff pack --model D80`).  Both commands default to these files; `--key` and `--cert` select others.

The private key is in the repository on purpose.  It is not a secret, and a signature made with it does not show who built a package.  The signature is provenance, not a gate: it records which build made a package and detects corruption, and the recovery built here installs any package left for it, signed or not.

To make another pair:

```sh
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out d80-ota.key.pem
openssl req -new -x509 -sha256 -key d80-ota.key.pem -days 36500 \
    -subj "/O=Open Desk Phone/CN=D80 OTA package key (not secret)" -out d80-ota.x509.pem
```
