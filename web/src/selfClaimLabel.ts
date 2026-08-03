// Self-claim button label — the arm/confirm two-step's full wording ladder,
// shared by the admin catalog rows and the game-detail modal (#51 PR D dedup).
// The already-owned warning only shows when a Steam identity is linked
// (ownedOnSteam = owned_by_ben && identity present at the call site): without
// the identity, ownership data is stale-prone and the plain confirm is honest.
export function selfClaimLabel(args: {
  armed: boolean;
  ownedOnSteam: boolean;
  requiresChoice: boolean;
  claiming: boolean;
}): string {
  if (!args.armed) {
    return args.claiming ? 'claiming…' : 'claim for me';
  }
  if (args.ownedOnSteam) {
    return args.requiresChoice
      ? 'you already own this on steam — spends 1 pick, sure?'
      : 'you already own this on steam — sure?';
  }
  return args.requiresChoice ? 'confirm? spends 1 pick' : 'confirm?';
}
