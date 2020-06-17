use super::super::{Command, Error};
use super::Encoder;
use crate::protocol;
use log::warn;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::fs;
use std::io::Write;
use tungstenite::Message;

#[derive(Debug)]
pub struct Payload<'a> {
	command: &'a Command,
}

impl<'a> Payload<'a> {
	fn new(command: &'a Command) -> Self {
		Self { command }
	}
}

impl<'a> Serialize for Payload<'a> {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		let mut state = serializer.serialize_struct("Command", 2)?;
		state.serialize_field("op", &self.command.op())?;
		state.serialize_field("d", self.command)?;
		state.end()
	}
}

pub struct JsonEncoder {
	file: fs::File,
}

impl JsonEncoder {
	pub fn new() -> Self {
		Self {
			file: fs::OpenOptions::new()
				.create(true)
				.append(true)
				.open("events.log")
				.unwrap(),
		}
	}
}

impl Encoder for JsonEncoder {
	fn encode(&mut self, command: Command) -> Message {
		let payload = Payload::new(&command);
		Message::Text(serde_json::to_string(&payload).unwrap())
	}

	fn decode(&mut self, message: Message) -> Result<protocol::Payload, Error> {
		match message {
			Message::Text(s) => {
				let p: protocol::Payload =
					serde_json::from_str(&s).map_err(|e| Error::DecodeJson(e))?;
				if !p.event.is_heartbeat_ack() {
					self.file.write_all(s.as_bytes()).unwrap();
					self.file.write_all("\n".as_bytes()).unwrap();
					self.file.sync_all().unwrap();
				}
				Ok(p)
			}
			Message::Close(frame) => Err(Error::Close(frame)),
			m => {
				warn!("Unexpected message type: {:?}", m);
				Err(Error::Decode)
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::gateway::Intents;
	use crate::Snowflake;
	use serde_json::Value;
	use std::str::FromStr;

	#[test]
	fn encode() {
		let mut encoder = JsonEncoder::new();
		let command = Command::heartbeat(42);
		let message = match encoder.encode(command) {
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

	#[test]
	fn decode() {
		let mut encoder = JsonEncoder::new();
		// Hello
		let message = Message::Text(
			"{\"t\":null,\"s\":null,\"op\":10,\"d\":{\"heartbeat_interval\":123456}}".into(),
		);
		let event = encoder.decode(message).unwrap().event;
		let hello = event.expect_hello().unwrap();
		assert_eq!(hello.heartbeat_interval, 123456);

		let message = Message::Text("{\"op\": 99, \"d\": {\"heartbeat_interval\": 123456}}".into());
		let event = encoder.decode(message).unwrap().event;
		assert_eq!(&event.expect_unknown().unwrap(), "99");

		// Ready
		let message = Message::Text("{\"t\":\"READY\",\"s\":1,\"op\":0,\"d\":{\"v\":6,\"user_settings\":{},\"user\":{\"verified\":true,\"username\":\"test\",\"mfa_enabled\":true,\"id\":\"292738120426342368\",\"flags\":0,\"email\":null,\"discriminator\":\"6948\",\"bot\":true,\"avatar\":null},\"session_id\":\"65063c9e155c35f380380e4c0251b1f1\",\"relationships\":[],\"private_channels\":[],\"presences\":[],\"guilds\":[{\"unavailable\":true,\"id\":\"191300962226790300\"}],\"application\":{\"id\":\"292738137426362368\",\"flags\":40960}}}".into());
		let event = encoder.decode(message).unwrap().event;
		let ready = event.expect_ready().unwrap();
		assert_eq!(&ready.user.username, "test");
		let mut it = ready.guilds.into_iter();
		assert_eq!(it.next(), Some(&Snowflake::from(191_300_962_226_790_300)));
		assert_eq!(it.next(), None);

		// Guild create
	}
}
