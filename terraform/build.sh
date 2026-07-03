#!/usr/bin/env bash
# Build the three lambda zips + the web bundle. Requires: cargo-lambda
# (https://www.cargo-lambda.info), zig or the cross toolchain it manages,
# node 22. Run from anywhere; paths are repo-relative.
set -euo pipefail
cd "$(dirname "$0")/.."

for bin in public-api admin-api fulfillment; do
  cargo lambda build --release --arm64 --output-format zip --bin "$bin"
done

mkdir -p terraform/artifacts
for bin in public-api admin-api fulfillment; do
  cp "target/lambda/$bin/bootstrap.zip" "terraform/artifacts/$bin.zip"
done

(cd web && npm ci && npm run build)

echo "artifacts:"
ls -la terraform/artifacts/
echo "web bundle: web/dist/"
