/**
 * Ticks, lots and atoms — the market's units, and the conversions between them.
 *
 * The mirror of `clob-book`'s `LotConfig`. Everything is exact integer arithmetic on
 * `bigint`: the market has no notion of a fractional lot, and a UI that introduces one
 * by dividing in floating point will submit an order for a size that does not exist.
 *
 * # The exactness invariant
 *
 * `tickSizeInQuoteLotsPerBaseUnit` is an integer multiple of `baseLotsPerBaseUnit`, so
 * one base lot at one tick is a whole number of quote lots. That is what makes every
 * fill's value exact rather than rounded, and it is checked on creation — a market that
 * violated it could not price its own trades.
 */

/** A market's tick and lot geometry. */
export interface LotConfig {
  readonly baseLotsPerBaseUnit: bigint;
  readonly tickSizeInQuoteLotsPerBaseUnit: bigint;
  readonly baseAtomsPerBaseLot: bigint;
  readonly quoteAtomsPerQuoteLot: bigint;
}

/** Why a lot configuration is not one any market could have been created with. */
export class LotConfigError extends Error {}

/**
 * Re-checks every invariant.
 *
 * Worth doing on anything decoded from an account: the bytes are cast rather than
 * parsed, so nothing has validated them, and a zero in the wrong field divides by zero
 * the first time something is priced.
 *
 * @throws {LotConfigError} if any field is zero or the tick size is not exact.
 */
export function validate(config: LotConfig): void {
  const fields: [string, bigint][] = [
    ["baseLotsPerBaseUnit", config.baseLotsPerBaseUnit],
    ["tickSizeInQuoteLotsPerBaseUnit", config.tickSizeInQuoteLotsPerBaseUnit],
    ["baseAtomsPerBaseLot", config.baseAtomsPerBaseLot],
    ["quoteAtomsPerQuoteLot", config.quoteAtomsPerQuoteLot],
  ];
  for (const [name, value] of fields) {
    if (value <= 0n) throw new LotConfigError(`${name} is ${value}`);
  }
  if (config.tickSizeInQuoteLotsPerBaseUnit % config.baseLotsPerBaseUnit !== 0n) {
    throw new LotConfigError(
      `a tick of ${config.tickSizeInQuoteLotsPerBaseUnit} quote lots per base unit does not ` +
        `divide evenly by ${config.baseLotsPerBaseUnit} base lots per base unit, so a fill ` +
        `would not have a whole quote value`,
    );
  }
}

/** The value of one base lot at a price of one tick. Exact, by the invariant above. */
export function quoteLotsPerBaseLotPerTick(config: LotConfig): bigint {
  return config.tickSizeInQuoteLotsPerBaseUnit / config.baseLotsPerBaseUnit;
}

/** The quote value of filling `baseLots` at `priceInTicks`. */
export function quoteLotsFor(config: LotConfig, priceInTicks: bigint, baseLots: bigint): bigint {
  return priceInTicks * quoteLotsPerBaseLotPerTick(config) * baseLots;
}

/**
 * The largest whole size buyable at `priceInTicks` with `quoteLots`.
 *
 * Rounds down, so the result always costs at most the budget.
 */
export function baseLotsFor(config: LotConfig, priceInTicks: bigint, quoteLots: bigint): bigint {
  const costPerBaseLot = priceInTicks * quoteLotsPerBaseLotPerTick(config);
  if (costPerBaseLot === 0n) {
    throw new RangeError("a price of zero makes the affordable size unbounded");
  }
  return quoteLots / costPerBaseLot;
}

/** Base lots to raw token atoms. */
export function baseAtoms(config: LotConfig, baseLots: bigint): bigint {
  return baseLots * config.baseAtomsPerBaseLot;
}

/** Quote lots to raw token atoms. */
export function quoteAtoms(config: LotConfig, quoteLots: bigint): bigint {
  return quoteLots * config.quoteAtomsPerQuoteLot;
}

/** Raw atoms to base lots, rounding down. Any remainder is dust the depositor keeps. */
export function baseLotsFromAtoms(config: LotConfig, atoms: bigint): bigint {
  return atoms / config.baseAtomsPerBaseLot;
}

/** Raw atoms to quote lots, rounding down. */
export function quoteLotsFromAtoms(config: LotConfig, atoms: bigint): bigint {
  return atoms / config.quoteAtomsPerQuoteLot;
}

/**
 * A price in ticks, rendered as a decimal string in quote units per base unit.
 *
 * For display only. It is the one place a fraction is correct — a human reads $150.25,
 * not 150250 ticks — and it returns a string rather than a number so the value cannot be
 * fed back into arithmetic that expects exactness.
 *
 * @param quoteDecimals the quote mint's decimals, which the mint account records.
 */
export function formatPrice(
  config: LotConfig,
  priceInTicks: bigint,
  quoteDecimals: number,
): string {
  // A tick is `tickSizeInQuoteLotsPerBaseUnit` quote lots per base unit; converting to
  // atoms and then to units is what puts it in the denomination a person recognises.
  const atomsPerBaseUnit = priceInTicks * config.tickSizeInQuoteLotsPerBaseUnit * config.quoteAtomsPerQuoteLot;
  return formatFixed(atomsPerBaseUnit, quoteDecimals);
}

/**
 * A size in base lots, rendered as a decimal string in base units.
 *
 * @param baseDecimals the base mint's decimals.
 */
export function formatSize(config: LotConfig, baseLots: bigint, baseDecimals: number): string {
  return formatFixed(baseAtoms(config, baseLots), baseDecimals);
}

/** Renders an integer number of atoms as a decimal string, without floating point. */
function formatFixed(atoms: bigint, decimals: number): string {
  if (decimals === 0) return atoms.toString();
  const scale = 10n ** BigInt(decimals);
  const whole = atoms / scale;
  const fraction = (atoms % scale).toString().padStart(decimals, "0").replace(/0+$/, "");
  return fraction === "" ? whole.toString() : `${whole}.${fraction}`;
}
