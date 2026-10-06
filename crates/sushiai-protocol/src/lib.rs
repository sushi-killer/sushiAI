//! Wire protocol shared by the daemon, the holder and clients.
//! No IO and no async runtime: bytes in, values out.

mod frame;
mod methods;
mod rpc;

pub use frame::{encode, Decoder, Frame, FrameError, MAX_FRAME};
pub use methods::*;
pub use rpc::{Message, Notification, Request, Response, RpcError};
