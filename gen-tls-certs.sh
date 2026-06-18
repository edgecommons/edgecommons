#!/usr/bin/env bash
# Generate throwaway TLS test certificates for the ggcommons secure-connection
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

# --- CA ---
openssl genrsa -out ca.key 2048
openssl req -x509 -new -nodes -key ca.key -sha256 -days 3650 \
  -subj "/CN=ggcommons-test-ca" -out ca.crt

# --- Server cert (CN/SAN localhost) ---
openssl genrsa -out server.key 2048
openssl req -new -key server.key -subj "/CN=localhost" -out server.csr
cat > server.ext <<'EOF'
subjectAltName = DNS:localhost,IP:127.0.0.1
extendedKeyUsage = serverAuth
EOF
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -out server.crt -days 3650 -sha256 -extfile server.ext

# --- Client cert (mutual TLS) ---
openssl genrsa -out client.key 2048
openssl req -new -key client.key -subj "/CN=ggcommons-test-client" -out client.csr
cat > client.ext <<'EOF'
extendedKeyUsage = clientAuth
EOF
openssl x509 -req -in client.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -out client.crt -days 3650 -sha256 -extfile client.ext

rm -f server.csr client.csr server.ext client.ext
echo "Generated test certs in $DIR:"
ls -1 "$DIR"
