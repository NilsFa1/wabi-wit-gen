//! Connection runtime: request/response correlation over one long-lived
//! socket, and the event hub.
//!
//!   request id --> oneshot::Sender --(WebSocket)--> response id --> oneshot::Receiver

mod connection;
mod correlator;
mod events;

pub use connection::Connection;
pub use correlator::Correlator;
pub use events::EventHub;
