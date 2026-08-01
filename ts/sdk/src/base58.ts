/**
 * Base58, the encoding Solana addresses are written in.
 *
 * Here rather than from a dependency because it is forty lines of arithmetic, and the
 * alternative is taking a package — and its supply chain — into an SDK that otherwise
 * has none. A wallet address is the last thing that should pass through code nobody in
 * this repository has read.
 *
 * Bitcoin's alphabet: no `0`, `O`, `I` or `l`, so an address cannot be misread aloud or
 * mistyped between characters that look alike.
 */

const ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/** Reverse lookup, built once. -1 marks a character outside the alphabet. */
const VALUES: Int8Array = (() => {
  const values = new Int8Array(128).fill(-1);
  for (let i = 0; i < ALPHABET.length; i++) {
    values[ALPHABET.charCodeAt(i)] = i;
  }
  return values;
})();

/** Bytes in a Solana address. */
export const ADDRESS_BYTES = 32;

/** Encodes bytes as base58. */
export function encodeBase58(bytes: Uint8Array): string {
  // Leading zero bytes carry no magnitude, so they survive the base conversion only if
  // they are counted separately and re-emitted as '1's.
  let zeros = 0;
  while (zeros < bytes.length && bytes[zeros] === 0) zeros++;

  // log(256)/log(58) is about 1.365; 138/100 is that with room to spare.
  const size = Math.ceil(((bytes.length - zeros) * 138) / 100) + 1;
  const digits = new Uint8Array(size);
  let length = 0;

  for (let i = zeros; i < bytes.length; i++) {
    let carry = bytes[i]!;
    let used = 0;
    for (let k = size - 1; (carry !== 0 || used < length) && k >= 0; k--, used++) {
      carry += 256 * digits[k]!;
      digits[k] = carry % 58;
      carry = (carry / 58) | 0;
    }
    length = used;
  }

  let start = size - length;
  while (start < size && digits[start] === 0) start++;

  let out = "1".repeat(zeros);
  for (let i = start; i < size; i++) out += ALPHABET[digits[i]!];
  return out;
}

/**
 * Decodes base58 into bytes.
 *
 * @throws if the string contains a character outside the alphabet.
 */
export function decodeBase58(text: string): Uint8Array {
  if (text.length === 0) return new Uint8Array(0);

  let zeros = 0;
  while (zeros < text.length && text[zeros] === "1") zeros++;

  const size = Math.ceil(((text.length - zeros) * 733) / 1000) + 1;
  const bytes = new Uint8Array(size);
  let length = 0;

  for (let i = zeros; i < text.length; i++) {
    const code = text.charCodeAt(i);
    const value = code < 128 ? VALUES[code]! : -1;
    if (value < 0) {
      throw new Error(`not base58: ${JSON.stringify(text[i])} at index ${i}`);
    }
    let carry = value;
    let used = 0;
    for (let k = size - 1; (carry !== 0 || used < length) && k >= 0; k--, used++) {
      carry += 58 * bytes[k]!;
      bytes[k] = carry % 256;
      carry = (carry / 256) | 0;
    }
    length = used;
  }

  let start = size - length;
  while (start < size && bytes[start] === 0) start++;

  const out = new Uint8Array(zeros + (size - start));
  out.set(bytes.subarray(start), zeros);
  return out;
}

/**
 * Decodes an address, insisting it is the right length.
 *
 * A 31-byte address is not a shorter address; it is a typo that would otherwise reach
 * the chain as a real account nobody controls.
 *
 * @throws if the string is not 32 bytes of base58.
 */
export function decodeAddress(address: string): Uint8Array {
  const bytes = decodeBase58(address);
  if (bytes.length !== ADDRESS_BYTES) {
    throw new Error(`${address} decodes to ${bytes.length} bytes, expected ${ADDRESS_BYTES}`);
  }
  return bytes;
}
