use super::Codec;
use crate::GatewayError;
use async_tungstenite::tungstenite::Message;
use log::warn;
use std::fs;
use std::io::Write;
use std::marker::PhantomData;

pub struct JsonCodec<S, D> {
	file: Option<fs::File>,
	s: PhantomData<S>,
	d: PhantomData<D>,
}

impl<S, D> JsonCodec<S, D> {
	pub fn new(file: Option<&'static str>) -> Self {
		Self {
			file: file.map(|f| {
				fs::OpenOptions::new()
					.create(true)
					.append(true)
					.open(f)
					.unwrap()
			}),
			s: PhantomData,
			d: PhantomData,
		}
	}
}

impl<S, D> Codec<S, D> for JsonCodec<S, D>
where
	S: serde::Serialize + Send + Sync,
	for<'a> D: serde::Deserialize<'a> + Send + Sync + std::fmt::Debug,
{
	fn encode(&mut self, command: S) -> Result<Message, GatewayError> {
		let s = serde_json::to_string(&command)?;
		if let Some(file) = &mut self.file {
			file.write_all("OUT: ".as_bytes()).unwrap();
			file.write_all(s.as_bytes()).unwrap();
			file.write_all("\n".as_bytes()).unwrap();
			file.sync_all().unwrap();
		}
		Ok(Message::Text(s))
	}

	fn decode(&mut self, message: Message) -> Result<D, GatewayError> {
		match message {
			Message::Text(s) => {
				if let Some(file) = &mut self.file {
					file.write_all("IN: ".as_bytes()).unwrap();
					file.write_all(s.as_bytes()).unwrap();
					file.write_all("\n".as_bytes()).unwrap();
					file.sync_all().unwrap();
				}
				serde_json::from_str(&s).map_err(|e| e.into())
			}
			Message::Close(frame) => Err(GatewayError::Close(frame)),
			m => {
				warn!("Unexpected message type: {:?}", m);
				Err(GatewayError::Decode)
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use discord_types::command;
	use discord_types::{Command, Intents, Payload};
	use serde_json::Value;
	use std::str::FromStr;

	#[test]
	fn encode() {
		let mut encoder = JsonCodec::<Command, Payload>::new(None);
		let command = command::Heartbeat { sequence: 42 };
		let message = match encoder.encode(command.into()).unwrap() {
			Message::Text(m) => m,
			e => panic!("Unexpected message variant: {:?}", e),
		};
		let value = Value::from_str(&message).unwrap();
		assert_eq!(value, Value::from_str("{\"op\": 1, \"d\": 42}").unwrap());
	}

	#[test]
	fn encode_intent() {
		let intent = Intents::GUILD_ALL;
		let value = serde_json::to_value(intent).unwrap();
		assert_eq!(value, Value::from_str("4095").unwrap());
	}
}
