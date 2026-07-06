#!/usr/bin/env bash
# Generate throwaway TLS test certificates for the edgecommons secure-connection
# integration tests (shared by the Java, Python, and Rust libraries): a CA, a
# server cert for localhost (used by the broker), and a client cert (used for
# mutual TLS). Output goes to ./tls-certs/. Safe to re-run.
#
# These are TEST-ONLY certs — never use them in production; ./tls-certs is gitignored.
set -euo pipefail
# Stop Git Bash (MSYS) from rewriting the openssl -subj "/CN=..." arguments into
# Windows paths.
export MSYS_NO_PATHCONV=1
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/tls-certs"
mkdir -p "$DIR"
cd "$DIR"

# --- CA --- (basicConstraints + keyUsage are required; strict TLS stacks, e.g.
# Python 3.13+/OpenSSL 3, reject a CA cert that lacks the keyCertSign key usage)
openssl genrsa -out ca.key 2048
openssl req -x509 -new -nodes -key ca.key -sha256 -days 3650 \
  -subj "/CN=edgecommons-test-ca" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" \
  -out ca.crt

# --- Server cert (CN/SAN localhost) ---
openssl genrsa -out server.key 2048
openssl req -new -key server.key -subj "/CN=localhost" -out server.csr
cat > server.ext <<'EOF'
basicConstraints = CA:FALSE
keyUsage = critical,digitalSignature,keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost,IP:127.0.0.1
EOF
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -out server.crt -days 3650 -sha256 -extfile server.ext

# --- Client cert (mutual TLS) ---
openssl genrsa -out client.key 2048
openssl req -new -key client.key -subj "/CN=edgecommons-test-client" -out client.csr
cat > client.ext <<'EOF'
basicConstraints = CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = clientAuth
EOF
openssl x509 -req -in client.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -out client.crt -days 3650 -sha256 -extfile client.ext

rm -f server.csr client.csr server.ext client.ext
echo "Generated test certs in $DIR:"
ls -1 "$DIR"
