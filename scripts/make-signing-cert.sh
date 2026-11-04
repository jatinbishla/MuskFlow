#!/usr/bin/env bash
# Creates a self-signed code-signing certificate, "Umless Local Signing", in your login
# keychain. scripts/make-app.sh signs with it when it exists, so macOS recognises every
# rebuild as the same app and keeps its Input Monitoring and Accessibility permissions.
#
# It stays on this Mac and is only good for running your own builds. To ship Umless
# to other people, sign with an Apple Developer ID instead.
#
# Remove it any time: Keychain Access → login → My Certificates → Umless Local Signing → Delete.
set -euo pipefail

NAME="Umless Local Signing"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if security find-certificate -c "$NAME" "$KEYCHAIN" >/dev/null 2>&1; then
  echo "\"$NAME\" already exists."
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
PASS="$(openssl rand -hex 16)"

cat > "$TMP/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF

/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout "$TMP/key.pem" -out "$TMP/cert.pem" -config "$TMP/cert.cnf" 2>/dev/null
/usr/bin/openssl pkcs12 -export -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
  -name "$NAME" -out "$TMP/id.p12" -passout "pass:$PASS"

# -T: let codesign use the key without asking each time.
security import "$TMP/id.p12" -k "$KEYCHAIN" -P "$PASS" -T /usr/bin/codesign >/dev/null
echo "Created \"$NAME\" in your login keychain."
