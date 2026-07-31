//! Trader identity, balances, and the seat table.

mod key;
mod state;
mod table;

pub use key::TraderKey;
pub use state::TraderState;
pub use table::{NO_SEAT, SeatIndex, TraderTable};
