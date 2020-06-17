use super::{Command, Error};
use crate::protocol::Payload;
pub use json::JsonEncoder;
use tungstenite::Message;

mod json;

pub trait Encoder: Sync + Send {
	fn encode(&mut self, command: Command) -> Message;
	fn decode(&mut self, message: Message) -> Result<Payload, Error>;
}
