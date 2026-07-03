#!/usr/bin/env bash
# Publish the SPA: sync web/dist to the site bucket + invalidate CloudFront.
# Ben runs this after `terraform apply` and after any web change. His creds.
set -euo pipefail
cd "$(dirname "$0")"

BUCKET="$(terraform output -raw s3_bucket_id)"
DIST_ID="$(terraform output -raw cloudfront_distribution_id)"

test -d ../web/dist || { echo "web/dist missing — run ./build.sh first" >&2; exit 1; }

aws s3 sync ../web/dist "s3://$BUCKET" --delete
aws cloudfront create-invalidation --distribution-id "$DIST_ID" --paths "/*"
echo "deployed to https://$(terraform output -raw site_url)"
