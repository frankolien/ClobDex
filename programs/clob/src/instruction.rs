//! The program's instruction set and its wire format.
//!
//! # Encoding
//!
//! One discriminant byte, then fixed-layout little-endian fields. Fields are read one at
//! a time rather than cast wholesale, because Solana hands instruction data over at an
//! arbitrary alignment and a `Pod` cast of a `u64` field would be undefined behaviour
//! roughly seven times out of eight. Reading with `from_le_bytes` is both correct and
//! cheaper than any serialisation framework.
//!
//! Discriminants are part of the public interface and must never be renumbered.

use clob_book::{BaseLots, FIFOOrderId, QuoteLots, Side, Ticks};
use clob_engine::{OrderPacket, PostOnlyRejection, SelfTradeBehavior};
use pinocchio::error::ProgramError;

use crate::error::ClobError;

/// Instruction discriminants.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Discriminant {
    /// Create a market in a pre-allocated account.
    InitializeMarket = 0,
    /// Claim a seat, or return the existing one.
    ClaimSeat = 1,
    /// Move tokens into the market and credit a seat.
    Deposit = 2,
    /// Debit a seat and move tokens out.
    Withdraw = 3,
    /// Submit an order.
    PlaceOrder = 4,
    /// Cancel a resting order.
    CancelOrder = 5,
    /// Shrink a resting order.
    ReduceOrder = 6,
    /// Cancel up to `limit` of the caller's orders on one side.
    CancelAllOrders = 7,
    /// Sweep accrued fees to the fee recipient.
    CollectFees = 8,
    /// Event sink. Emitted by this program calling back into itself; the handler does
    /// nothing, and exists only so the payload lands in the transaction's inner
    /// instruction data where it cannot be truncated.
    LogEvent = 9,
    /// Deposit, match and withdraw in one instruction, for callers who hold no balance
    /// on the market.
    Swap = 10,
    /// Return an empty seat to the market. Permissionless.
    EvictSeat = 11,
}

impl Discriminant {
    /// Reads the leading discriminant byte.
    ///
    /// # Errors
    ///
    /// [`ClobError::InstructionDataTooShort`] or [`ClobError::UnknownInstruction`].
    pub const fn parse(byte: u8) -> Result<Self, ClobError> {
        match byte {
            0 => Ok(Self::InitializeMarket),
            1 => Ok(Self::ClaimSeat),
            2 => Ok(Self::Deposit),
            3 => Ok(Self::Withdraw),
            4 => Ok(Self::PlaceOrder),
            5 => Ok(Self::CancelOrder),
            6 => Ok(Self::ReduceOrder),
            7 => Ok(Self::CancelAllOrders),
            8 => Ok(Self::CollectFees),
            9 => Ok(Self::LogEvent),
            10 => Ok(Self::Swap),
            11 => Ok(Self::EvictSeat),
            _ => Err(ClobError::UnknownInstruction),
        }
    }
}

/// A cursor over instruction data that reads fixed-width little-endian fields.
pub struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    /// Starts reading after the discriminant byte.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ProgramError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ClobError::InstructionDataTooShort)?;
        if end > self.bytes.len() {
            return Err(ClobError::InstructionDataTooShort.into());
        }
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    /// Reads one byte.
    ///
    /// # Errors
    ///
    /// [`ClobError::InstructionDataTooShort`].
    pub fn u8(&mut self) -> Result<u8, ProgramError> {
        Ok(self.take(1)?[0])
    }

    /// Reads a little-endian `u32`.
    ///
    /// # Errors
    ///
    /// [`ClobError::InstructionDataTooShort`].
    pub fn u32(&mut self) -> Result<u32, ProgramError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Reads a little-endian `u64`.
    ///
    /// # Errors
    ///
    /// [`ClobError::InstructionDataTooShort`].
    pub fn u64(&mut self) -> Result<u64, ProgramError> {
        let bytes = self.take(8)?;
        let mut buffer = [0u8; 8];
        buffer.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(buffer))
    }

    /// Reads a 32-byte address.
    ///
    /// # Errors
    ///
    /// [`ClobError::InstructionDataTooShort`].
    pub fn address(&mut self) -> Result<[u8; 32], ProgramError> {
        let bytes = self.take(32)?;
        let mut buffer = [0u8; 32];
        buffer.copy_from_slice(bytes);
        Ok(buffer)
    }

    /// Reads a side, encoded as `0` for bid and `1` for ask.
    ///
    /// # Errors
    ///
    /// [`ProgramError::InvalidInstructionData`] for any other value.
    pub fn side(&mut self) -> Result<Side, ProgramError> {
        match self.u8()? {
            0 => Ok(Side::Bid),
            1 => Ok(Side::Ask),
            _ => Err(ProgramError::InvalidInstructionData),
        }
    }

    /// Reads an order id: price then encoded sequence number.
    ///
    /// # Errors
    ///
    /// [`ClobError::InstructionDataTooShort`].
    pub fn order_id(&mut self) -> Result<FIFOOrderId, ProgramError> {
        let price = Ticks(self.u64()?);
        let sequence = self.u64()?;
        Ok(FIFOOrderId::from_encoded(price, sequence))
    }

    /// Reads an order packet.
    ///
    /// Layout: kind byte, side byte, price, size, then kind-specific fields. An
    /// unpriced market order is a priced IOC with the price flag cleared, so the packet
    /// stays fixed-width and the client does not have to branch on length.
    ///
    /// # Errors
    ///
    /// [`ClobError::InstructionDataTooShort`] or
    /// [`ProgramError::InvalidInstructionData`] for an unknown kind or policy.
    pub fn order_packet(&mut self) -> Result<OrderPacket, ProgramError> {
        let kind = self.u8()?;
        let side = self.side()?;
        let has_price = self.u8()? != 0;
        let price_in_ticks = Ticks(self.u64()?);
        let num_base_lots = BaseLots(self.u64()?);

        match kind {
            0 => Ok(OrderPacket::Limit {
                side,
                price_in_ticks,
                num_base_lots,
                self_trade_behavior: self.self_trade_behavior()?,
                match_limit: self.u32()?,
            }),
            1 => Ok(OrderPacket::PostOnly {
                side,
                price_in_ticks,
                num_base_lots,
                rejection: match self.u8()? {
                    0 => PostOnlyRejection::Reject,
                    1 => PostOnlyRejection::Slide,
                    _ => return Err(ProgramError::InvalidInstructionData),
                },
            }),
            2 => Ok(OrderPacket::ImmediateOrCancel {
                side,
                price_in_ticks: has_price.then_some(price_in_ticks),
                num_base_lots,
                min_base_lots_to_fill: BaseLots(self.u64()?),
                self_trade_behavior: self.self_trade_behavior()?,
                match_limit: self.u32()?,
            }),
            _ => Err(ProgramError::InvalidInstructionData),
        }
    }

    fn self_trade_behavior(&mut self) -> Result<SelfTradeBehavior, ProgramError> {
        match self.u8()? {
            0 => Ok(SelfTradeBehavior::DecrementTake),
            1 => Ok(SelfTradeBehavior::CancelProvide),
            2 => Ok(SelfTradeBehavior::Abort),
            _ => Err(ProgramError::InvalidInstructionData),
        }
    }

    /// Reads a base-lot amount.
    ///
    /// # Errors
    ///
    /// [`ClobError::InstructionDataTooShort`].
    pub fn base_lots(&mut self) -> Result<BaseLots, ProgramError> {
        Ok(BaseLots(self.u64()?))
    }

    /// Reads a quote-lot amount.
    ///
    /// # Errors
    ///
    /// [`ClobError::InstructionDataTooShort`].
    pub fn quote_lots(&mut self) -> Result<QuoteLots, ProgramError> {
        Ok(QuoteLots(self.u64()?))
    }

    /// Reads a trailing optional byte, returning `None` if the data ends here.
    ///
    /// Used for fields that only the receipt form of an instruction carries, so the
    /// common form stays byte-for-byte what it was.
    pub fn optional_u8(&mut self) -> Option<u8> {
        self.u8().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_refuses_to_run_past_the_end() {
        let mut reader = Reader::new(&[1, 2, 3]);
        assert!(reader.u8().is_ok());
        assert!(reader.u64().is_err());
    }

    #[test]
    fn order_ids_round_trip() {
        let id = FIFOOrderId::new(Side::Bid, Ticks(1234), 99);
        let mut bytes = std::vec::Vec::new();
        bytes.extend_from_slice(&id.price_in_ticks.as_u64().to_le_bytes());
        bytes.extend_from_slice(&id.order_sequence_number.to_le_bytes());

        assert_eq!(Reader::new(&bytes).order_id().unwrap(), id);
    }

    #[test]
    fn an_unpriced_ioc_decodes_as_a_market_order() {
        // kind=2, side=bid, has_price=0, price=0, size=7, min=0, stb=0, match_limit=64
        let mut bytes = std::vec![2u8, 0, 0];
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&7u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&64u32.to_le_bytes());

        let packet = Reader::new(&bytes).order_packet().unwrap();
        assert_eq!(packet.price_in_ticks(), None);
        assert_eq!(packet.num_base_lots(), BaseLots(7));
    }

    #[test]
    fn unknown_discriminants_and_policies_are_rejected() {
        assert_eq!(
            Discriminant::parse(200),
            Err(ClobError::UnknownInstruction)
        );
        let bytes = [9u8, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
        assert!(Reader::new(&bytes).order_packet().is_err());
    }
}
