# The D80's OTA package key

`d80-ota.key.pem` is an RSA private key (2048 bits, PKCS #8), and `d80-ota.x509.pem` is its self-signed certificate.  The build signs the D80's OTA package with them (`opendeskphone-image d80 package`, `nix build .#ota-d80`).

The private key is in the repository on purpose.  It is not a secret, and a signature made with it does not show who built a package.  A recovery that has this certificate among its keys installs a package from anyone.  The D80's recovery built here (`crates/opendeskphone-recovery`) has this certificate and no other, and `dphone update install` checks a package against it before the recovery does.  The certificate is compiled into both, so a new pair takes effect with the next build.

To make another pair:

```sh
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out d80-ota.key.pem
openssl req -new -x509 -sha256 -key d80-ota.key.pem -days 36500 \
    -subj "/O=Open Desk Phone/CN=D80 OTA package key (not secret)" -out d80-ota.x509.pem
```
