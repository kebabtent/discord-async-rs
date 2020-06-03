mod json;

use super::event;
use super::{Command, Error};
pub use json::JsonEncoder;
use tungstenite::Message;

pub trait Encoder: Sync + Send {
	fn encode(&mut self, command: Command) -> Message;
	fn decode(&mut self, message: Message) -> Result<event::Payload, Error>;
}
