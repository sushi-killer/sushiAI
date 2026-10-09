#!/bin/sh
# Creates a stable local code-signing identity ("sushiAI Local Signing") in the login
# keychain, so every local build of sushiAI and its daemon signs with the same identity.
# macOS privacy grants (Desktop, Full Disk Access) are then kept across rebuilds; with
# ad-hoc signing every build is a new app to macOS and it asks again.
# Run once per machine. The private key stays in the keychain; no file keeps it.
set -eu
NAME="sushiAI Local Signing"
if security find-identity -v -p codesigning | grep -q "$NAME"; then
  echo "\"$NAME\" already exists."
  exit 0
fi
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cat >"$dir/cfg" <<EOF
[req]
distinguished_name=dn
x509_extensions=ext
prompt=no
[dn]
CN=$NAME
[ext]
basicConstraints=critical,CA:false
keyUsage=critical,digitalSignature
extendedKeyUsage=critical,codeSigning
EOF
pass=$(openssl rand -hex 16)
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$dir/cfg" \
  -keyout "$dir/key.pem" -out "$dir/cert.pem" 2>/dev/null
openssl pkcs12 -export -legacy -inkey "$dir/key.pem" -in "$dir/cert.pem" \
  -out "$dir/id.p12" -passout "pass:$pass" 2>/dev/null ||
  openssl pkcs12 -export -inkey "$dir/key.pem" -in "$dir/cert.pem" \
    -out "$dir/id.p12" -passout "pass:$pass"
security import "$dir/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
  -P "$pass" -T /usr/bin/codesign
echo "macOS now asks for your password to trust the certificate for code signing."
security add-trusted-cert -r trustRoot -p codeSign \
  -k "$HOME/Library/Keychains/login.keychain-db" "$dir/cert.pem"
security find-identity -v -p codesigning | grep "$NAME"
