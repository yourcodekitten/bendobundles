# Product

## Register

brand

## Users

Two audiences, one moment that matters:

- **Friends receiving a gift link.** They arrive from a chat message ben sent them — often on a
  phone, no account, no context beyond "ben sent me this." Their job: see which games were chosen
  *for them*, claim one, and land on a working Humble gift key. They visit rarely and remember the
  feeling, not the UI.
- **Ben, the giver.** He curates ~15 years of Humble Bundle purchases through the admin surfaces
  (catalog, links, ops). His job is generosity logistics: find a game, cut an invite link, hand it
  to a friend, and trust the plumbing. The admin is a workbench that serves the gift — it shares
  the family warmth but never upstages the friend surface.

## Product Purpose

bendobundles puts ben's ~15 years of Humble Bundle purchases in one place and turns them into
gifts: invite links let friends claim games and instantly receive Humble gift links. It exists
because ben forgets his bundles exist 95% of the time — and because an unclaimed game is a gift
nobody got to open.

Success looks like: a friend opens their link and feels *chosen for*, the claim works on the first
try, and ben's library stops gathering dust.

## Brand Personality

**playful · cozy · nostalgic.**

The gift-unwrap is the emotional center: warm, a little ceremonial — someone picked these games
FOR you. The library reads as a game-collection attic full of treasures, not a database. The voice
is lowercase and affectionate (the ♡ in Landing.tsx is canon, not a typo); charm shows up in copy,
color, and pacing rather than volume. Fifteen years of bundles carry real gaming nostalgia — soft
edges, cover art, the feeling of finding something you forgot you loved.

## Anti-references

- **Storefront / commerce.** No Steam-store or e-commerce energy: no prices, no urgency, no
  "deals" visual language, no conversion patterns. It's a gift shelf, never a shop.
- **SaaS dashboard chrome.** No metric-card grids, sidebar-and-breadcrumb chrome, or BI-tool
  styling — not even on the admin, which is a workbench, not a dashboard.
- **Gamer-RGB edgelord.** No neon-on-black, aggressive angular "gaming brand" styling, or
  glow-everything. Nostalgic-cozy, not esports.
- **Corporate minimalism.** No sterile white-space-and-gray-text "tasteful" emptiness. Warmth
  over restraint; safe = invisible.

## Design Principles

1. **The unwrap is the product.** The claim moment gets the ceremony; every screen before it
   exists to clear the path there. Spend the craft budget where the friend's heart rate is.
2. **Chosen-for-you, never shopping.** Gift-shelf grammar throughout: curation over inventory,
   receiving over browsing. If a pattern would look at home in a store, replace it.
3. **Treasures, not rows.** Games are artifacts with stories — cover art, trailers, the year ben
   bought them — not table entries. Presentation honors the attic-of-treasures feeling.
4. **Warm surface, invisible machinery.** The serverless plumbing (lambdas, dynamo, humble
   reveals) never leaks into the friend's experience; errors speak the brand's voice, softly.
5. **Delight never gates.** Charm layers on top of a flow that works plain: thin fallbacks when
   enrichment hasn't landed, reduced-motion alternatives, everything claimable without the magic.

## Accessibility & Inclusion

Best effort, friends-and-family bar: no formal WCAG conformance gate, but keep the habits already
in the codebase — keyboard + focus management in dialogs (ClaimDialog / GameDetailModal pattern),
reduced-motion alternatives for any ceremony animation, and readable contrast on the dark theme.
Don't gate work on audits; don't ship anything a friend on a phone can't claim with.
