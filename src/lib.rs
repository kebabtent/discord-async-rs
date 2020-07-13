use self::gateway::Command;
use chrono::prelude::{DateTime, Utc};
use chrono::TimeZone;
pub use client::Client;
pub use discord::{Builder, Discord, Error};
use futures::channel::mpsc;
pub use gateway::{Connector, Gateway};
pub use guild::{Guild, GuildSeed};
pub use protocol::ProtocolError;
use serde::de::{Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;
pub use types::Event;

pub mod api;
mod client;
mod discord;
mod gateway;
mod guild;
pub mod protocol;
pub mod types;

pub(crate) type Send<T> = mpsc::Sender<T>;
pub(crate) type Recv<T> = mpsc::Receiver<T>;

const DISCORD_EPOCH: u64 = 1_420_070_400_000;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Snowflake(u64);

impl Snowflake {
	pub fn date_time(&self) -> DateTime<Utc> {
		let timestamp = (self.0 >> 22) + DISCORD_EPOCH;
		Utc.timestamp_millis(timestamp as i64)
	}

	pub fn worker(&self) -> u8 {
		((self.0 & 0x3E0000) >> 17) as u8
	}

	pub fn process(&self) -> u8 {
		((self.0 & 0x1F000) >> 12) as u8
	}

	pub fn increment(&self) -> u16 {
		(self.0 & 0xFFF) as u16
	}
}

impl fmt::Display for Snowflake {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		fmt::Display::fmt(&self.0, f)
	}
}

impl From<u64> for Snowflake {
	fn from(snowflake: u64) -> Self {
		Self(snowflake)
	}
}

impl FromStr for Snowflake {
	type Err = ParseIntError;
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let id = s.parse::<u64>()?;
		Ok(id.into())
	}
}

impl PartialEq<u64> for Snowflake {
	fn eq(&self, other: &u64) -> bool {
		self.0 == *other
	}
}

impl<'de> Deserialize<'de> for Snowflake {
	fn deserialize<D>(deserializer: D) -> Result<Snowflake, D::Error>
	where
		D: Deserializer<'de>,
	{
		struct SnowflakeVisitor;

		impl<'de> Visitor<'de> for SnowflakeVisitor {
			type Value = Snowflake;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("u64 or string storing a snowflake id")
			}

			fn visit_u64<E>(self, val: u64) -> Result<Snowflake, E>
			where
				E: serde::de::Error,
			{
				Ok(Snowflake(val))
			}

			fn visit_str<E>(self, val: &str) -> Result<Snowflake, E>
			where
				E: serde::de::Error,
			{
				val.parse()
					.map(|v| Snowflake(v))
					.map_err(|_| E::invalid_value(Unexpected::Str(val), &self))
			}
		}

		deserializer.deserialize_any(SnowflakeVisitor)
	}
}

impl Serialize for Snowflake {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		serializer.serialize_str(&format!("{}", self.0))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn snowflake() {
		let id = Snowflake(175_928_847_299_117_063);
		let time = Utc.ymd(2016, 04, 30).and_hms_milli(11, 18, 25, 796);
		assert_eq!(id.date_time(), time);
		assert_eq!(id.worker(), 1);
		assert_eq!(id.process(), 0);
		assert_eq!(id.increment(), 7);
	}
}
