/**
 * Getting an SDK instruction onto the chain.
 *
 * The SDK returns plain data — a program address, an account list and a `Uint8Array` — and
 * says its field names match `@solana/kit`'s so the adaptation is "usually the identity
 * function". {@link toKit} is where that claim is either true or a lie, so it is separated
 * out and tested rather than inlined into a click handler.
 *
 * Everything below it needs a wallet and a network and therefore cannot be tested here.
 * That split is deliberate: the part that can be verified without a browser is, and the
 * part that cannot is kept as thin as it can be made.
 */

import {
  type Address,
  AccountRole,
  type Instruction as KitInstruction,
  appendTransactionMessageInstructions,
  createSolanaRpc,
  createTransactionMessage,
  getBase58Decoder,
  pipe,
  setTransactionMessageFeePayerSigner,
  setTransactionMessageLifetimeUsingBlockhash,
  signAndSendTransactionMessageWithSigners,
} from "@solana/kit";
import type { TransactionSendingSigner } from "@solana/kit";

import type { Instruction as SdkInstruction } from "@clobdex/sdk";

/**
 * Converts one SDK instruction into the shape kit signs.
 *
 * Almost the identity function, and the "almost" is the whole reason this exists: the SDK
 * describes an account with two booleans, and kit describes the same four combinations
 * with one enum. Getting that mapping backwards produces a transaction that is rejected
 * for the wrong signers or, worse, one that is accepted having asked for write access it
 * did not need.
 */
export function toKit(instruction: SdkInstruction): KitInstruction {
  return {
    programAddress: instruction.programAddress as Address,
    accounts: instruction.accounts.map((account) => ({
      address: account.address as Address,
      role: roleOf(account.signer, account.writable),
    })),
    data: instruction.data,
  };
}

/** The four combinations of "must sign" and "may be modified", as kit spells them. */
export function roleOf(signer: boolean, writable: boolean): AccountRole {
  if (signer) return writable ? AccountRole.WRITABLE_SIGNER : AccountRole.READONLY_SIGNER;
  return writable ? AccountRole.WRITABLE : AccountRole.READONLY;
}

/** A signature, base58, as an explorer wants it. */
export function signatureToString(bytes: Uint8Array): string {
  return getBase58Decoder().decode(bytes);
}

/**
 * Signs and sends one transaction carrying `instructions`, in order.
 *
 * The blockhash is fetched immediately before signing rather than cached. A stale one
 * produces a transaction the cluster rejects outright, which is a confusing failure to
 * debug from a UI, and the fetch costs one round trip against a signing prompt a person is
 * about to spend seconds on anyway.
 *
 * The wallet both signs and sends — that is what a `TransactionSendingSigner` is — so this
 * never handles a signed transaction itself.
 */
export async function send(
  rpcUrl: string,
  signer: TransactionSendingSigner,
  instructions: readonly SdkInstruction[],
): Promise<string> {
  const rpc = createSolanaRpc(rpcUrl);
  const { value: blockhash } = await rpc.getLatestBlockhash({ commitment: "confirmed" }).send();

  const message = pipe(
    createTransactionMessage({ version: 0 }),
    (draft) => setTransactionMessageFeePayerSigner(signer, draft),
    (draft) => setTransactionMessageLifetimeUsingBlockhash(blockhash, draft),
    (draft) => appendTransactionMessageInstructions(instructions.map(toKit), draft),
  );

  return signatureToString(await signAndSendTransactionMessageWithSigners(message));
}
