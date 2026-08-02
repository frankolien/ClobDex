/**
 * Reading how many atoms make one token.
 *
 * The indexer serves mint addresses, not metadata, so decimals have to come from the chain.
 * Without them every price and size on screen is off by whatever the real exponent is —
 * silently, and by orders of magnitude, which is the worst kind of wrong a trading screen
 * can be.
 *
 * The SPL mint layout is fixed at 82 bytes and has not changed:
 *
 * ```text
 * 0..4    COption tag for the mint authority
 * 4..36   mint authority
 * 36..44  supply, u64 little-endian
 * 44      decimals, u8          <- the only field this needs
 * 45      is_initialized, bool
 * 46..50  COption tag for the freeze authority
 * 50..82  freeze authority
 * ```
 *
 * Token-2022 mints are longer, because extensions are appended after the base layout — but
 * the first 82 bytes are the same, so this reads them identically and does not care.
 */

/** Bytes in the base SPL mint account. Token-2022 mints are this plus extensions. */
export const MINT_LENGTH = 82;

/** Offset of the decimals byte. */
const DECIMALS_OFFSET = 44;

/** Offset of the initialisation flag. */
const INITIALIZED_OFFSET = 45;

export class MintDecodeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "MintDecodeError";
  }
}

/**
 * Reads the decimals of an SPL mint from its account data.
 *
 * Refuses an uninitialised mint rather than reading a zero out of it. A zeroed account
 * decodes to "0 decimals", which is a perfectly plausible answer — and displaying every
 * amount a billion times too large is worse than displaying nothing.
 */
export function decimalsOf(data: Uint8Array): number {
  if (data.length < MINT_LENGTH) {
    throw new MintDecodeError(`a mint is at least ${MINT_LENGTH} bytes, got ${data.length}`);
  }
  if (data[INITIALIZED_OFFSET] !== 1) {
    throw new MintDecodeError("this account is not an initialised mint");
  }

  const decimals = data[DECIMALS_OFFSET];
  if (decimals === undefined || decimals > 18) {
    throw new MintDecodeError(`implausible decimals: ${decimals}`);
  }
  return decimals;
}

/** Decodes standard base64 into bytes. What an RPC returns account data as. */
export function fromBase64(text: string): Uint8Array {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

/**
 * Fetches decimals for several mints at once.
 *
 * One `getMultipleAccounts` rather than a request per mint: a markets table has two per
 * row, and the answers never change, so this is called once and cached for the session.
 * A mint that cannot be read is left out rather than defaulted — the caller decides what
 * to do about a missing one, and quietly substituting nine is how a wrong number ships.
 */
export async function fetchDecimals(
  rpcUrl: string,
  mints: readonly string[],
  signal?: AbortSignal,
): Promise<Map<string, number>> {
  const found = new Map<string, number>();
  const unique = [...new Set(mints)];
  if (unique.length === 0) return found;

  const response = await fetch(rpcUrl, {
    method: "POST",
    headers: { "content-type": "application/json" },
    signal: signal ?? null,
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "getMultipleAccounts",
      // `base64` rather than `jsonParsed`: the layout is eighty-two fixed bytes and reading
      // one of them is cheaper and more predictable than asking the node to parse it.
      params: [unique, { encoding: "base64", commitment: "confirmed" }],
    }),
  });

  if (!response.ok) throw new Error(`RPC returned ${response.status}`);

  const body = (await response.json()) as {
    result?: { value: ({ data: [string, string] } | null)[] };
    error?: { message: string };
  };
  if (body.error) throw new Error(body.error.message);

  const values = body.result?.value ?? [];
  for (const [index, account] of values.entries()) {
    const mint = unique[index];
    if (!mint || !account) continue;
    try {
      found.set(mint, decimalsOf(fromBase64(account.data[0])));
    } catch {
      // Left absent on purpose. See the note above.
    }
  }
  return found;
}
