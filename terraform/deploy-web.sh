#!/usr/bin/env bash
# Publish the SPA: sync web/dist to the site bucket + invalidate CloudFront.
# Ben runs this after `terraform apply` and after any web change. His creds.
set -euo pipefail
cd "$(dirname "$0")"

BUCKET="$(terraform output -raw s3_bucket_id)"
DIST_ID="$(terraform output -raw cloudfront_distribution_id)"

test -d ../web/dist || { echo "web/dist missing — run ./build.sh first" >&2; exit 1; }
# The unfurl lambda serves this file verbatim: a dist built before the og-marker
# commit would ship marker-less HTML that only the runtime EMF witness can see.
# Prevention beats detection — refuse to sync a stale dist.
grep -q 'og:begin' ../web/dist/index.html \
  || { echo "web/dist/index.html has no og:begin marker — stale dist; run ./build.sh" >&2; exit 1; }

# Hashed bundles are retained (no --delete under assets/): the unfurl lambda's
# 60s template cache + CF's 60s unfurl TTL can serve a pre-deploy index.html for
# up to ~2 min after this sync, and deleting old hashes in that window would 404
# the scripts of a page a friend just opened. Old hashes are tiny; prune rarely.
aws s3 sync ../web/dist/assets "s3://$BUCKET/assets"
aws s3 sync ../web/dist "s3://$BUCKET" --delete --exclude "assets/*"
aws cloudfront create-invalidation --distribution-id "$DIST_ID" --paths "/*"
echo "deployed to https://$(terraform output -raw site_url)"
