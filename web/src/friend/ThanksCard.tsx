import { useState } from 'react';
import { sendThanks } from '../api';

/** Mirrors the server's THANK_NOTE_MAX_CHARS (public-api). The textarea's
 * maxLength counts UTF-16 units, which can only over-count vs the server's
 * scalar count — typing can never produce a note the server rejects as long. */
const MAX_CHARS = 500;

interface ThanksCardProps {
  token: string;
  /** Server-echoed note when one was already sent — a revisit renders the
   * sent state directly, no compose. */
  thankNote?: string;
}

/** The friend's one word back to ben — gift_note's return path (write-once;
 * the server holds the first word). The parent gates rendering on claims
 * being present: thanks is the echo of an unwrap, not a guestbook. */
function SentNote({ note }: { note: string }) {
  return (
    <section aria-label="your thank-you" className="px-6 py-2">
      <div className="max-w-[34rem] rounded bg-floor px-4 py-3">
        <p className="max-w-[60ch] text-sm italic text-give-soft">
          &ldquo;{note}&rdquo;{' '}
          <span className="font-pixel not-italic text-xs text-dust">
            &mdash; you, delivered to ben ♡
          </span>
        </p>
      </div>
    </section>
  );
}

export function ThanksCard({ token, thankNote }: ThanksCardProps) {
  const [note, setNote] = useState('');
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Own send wins for immediacy, else whatever the server last said. Deriving
  // (not seeding state from the prop) means a refetch that surfaces a note sent
  // in ANOTHER tab flips this tab to the sent view too — mount-time seeding
  // would ignore prop updates forever (review pass 1).
  const effectiveSent = sent ?? thankNote ?? null;
  if (effectiveSent !== null) {
    return <SentNote note={effectiveSent} />;
  }

  // Counted in UTF-16 units to agree with maxLength, which the browser enforces
  // in the same units — else 250 pasted emoji lock the box while the counter
  // still claims 250 left. The server's scalar count is never larger than the
  // UTF-16 count, so anything the box admits, the server admits.
  const remaining = MAX_CHARS - note.length;

  const submit = async () => {
    setSending(true);
    setError(null);
    const result = await sendThanks(token, note.trim());
    setSending(false);
    if (result.kind === 'sent') {
      setSent(result.thank_note);
    } else {
      setError(result.message);
    }
  };

  return (
    <section aria-label="say thanks to ben" className="px-6 py-2">
      <div className="max-w-[34rem] rounded bg-floor px-4 py-3">
        <h2 className="font-pixel text-sm text-give-soft">say thanks to ben ♡</h2>
        <p className="mt-1 text-xs text-dust">
          he opened his stash for you — leave him a word back. you get one, so make it count~
        </p>
        <textarea
          value={note}
          onChange={(e) => setNote(e.target.value)}
          maxLength={MAX_CHARS}
          rows={3}
          disabled={sending}
          aria-label="your thank-you note"
          placeholder="omg thank you!!"
          className="mt-2 w-full rounded border border-line bg-room px-3 py-2 text-sm text-ink placeholder:text-dust-faint"
        />
        <div className="mt-1 flex items-center gap-3">
          <button
            type="button"
            onClick={() => void submit()}
            disabled={sending || note.trim() === ''}
            className="rounded bg-control px-4 py-2 text-sm hover:bg-control-bright disabled:cursor-not-allowed disabled:opacity-50"
          >
            {sending ? 'sending…' : 'send it ♡'}
          </button>
          <span className="text-xs text-dust-faint" aria-hidden="true">
            {remaining}
          </span>
          {error !== null && (
            <span role="alert" className="text-xs text-dust">
              {error}
            </span>
          )}
        </div>
      </div>
    </section>
  );
}
