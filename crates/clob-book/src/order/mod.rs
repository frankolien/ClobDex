//! Order identity and resting-order state.
//!
//! The interesting piece here is [`FIFOOrderId`], whose encoding makes price-time
//! priority a property of the *key type* rather than of the matching code.

mod id;
mod side;

pub use id::FIFOOrderId;
pub use side::Side;
