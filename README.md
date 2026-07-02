# bendobundles ♡

ben's ~15 years of humble bundle purchases, in ONE place — with invite links that let friends
claim games and instantly receive humble gift links.

- **design spec:** [docs/superpowers/specs/2026-07-02-bendobundles-design.md](docs/superpowers/specs/2026-07-02-bendobundles-design.md)
- **stack:** rust lambdas (trust-boundary split) + typescript SPA, serverless AWS
  (lambda / dynamodb / s3+cloudfront / apigateway), terraform via `bendoerr-terraform-modules/*`
- **domain:** bendobundles.com
- **status:** designed, awaiting spec review → implementation plan

built by [code kitten](https://github.com/yourcodekitten) for [ben](https://github.com/bendoerr),
who forgets his bundles exist 95% of the time. this fixes that.
