use crate::Snowflake;
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Deserialize)]
pub struct Hello {
	pub heartbeat_interval: u64,
}

#[derive(Debug, Deserialize)]
pub struct Ready {
	pub v: u8,
	pub user: User,
	pub session_id: String,
	pub guilds: GuildList,
	pub shard: Option<(u8, u8)>,
}

#[derive(Debug)]
pub struct GuildList {
	inner: HashSet<Snowflake>,
}

impl GuildList {
	fn new() -> Self {
		Self {
			inner: HashSet::new(),
		}
	}

	fn len(&self) -> usize {
		self.inner.len()
	}

	fn contains(&self, value: Snowflake) -> bool {
		self.inner.contains(&value)
	}

	fn insert(&mut self, id: Snowflake) -> bool {
		self.inner.insert(id)
	}
}

impl<'a> IntoIterator for &'a GuildList {
	type Item = &'a Snowflake;
	type IntoIter = std::collections::hash_set::Iter<'a, Snowflake>;

	#[inline]
	fn into_iter(self) -> Self::IntoIter {
		(&self.inner).into_iter()
	}
}

impl<'de> Deserialize<'de> for GuildList {
	fn deserialize<D>(deserializer: D) -> Result<GuildList, D::Error>
	where
		D: Deserializer<'de>,
	{
		#[derive(Deserialize)]
		struct Guild {
			id: Snowflake,
		}

		struct GuildListVisitor;

		impl<'de> Visitor<'de> for GuildListVisitor {
			type Value = GuildList;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("struct GuildList")
			}

			fn visit_seq<V>(self, mut seq: V) -> Result<GuildList, V::Error>
			where
				V: SeqAccess<'de>,
			{
				let mut list = GuildList::new();
				while let Some(guild) = seq.next_element::<Guild>()? {
					list.insert(guild.id);
				}
				Ok(list)
			}
		}

		deserializer.deserialize_seq(GuildListVisitor)
	}
}

#[derive(Debug, Deserialize)]
pub struct User {
	pub id: Snowflake,
	pub username: String,
	pub discriminator: String,
	pub avatar: Option<String>,
	#[serde(default)]
	pub bot: Option<bool>,
	#[serde(default)]
	pub system: Option<bool>,
	#[serde(default)]
	pub mfa_enabled: Option<bool>,
	#[serde(default)]
	pub locale: Option<String>,
	#[serde(default)]
	pub verified: Option<bool>,
	#[serde(default)]
	pub email: Option<String>,
	#[serde(default)]
	pub flags: Option<u64>,
	#[serde(default)]
	pub premium_type: Option<u64>,
	#[serde(default)]
	pub public_flags: Option<u64>,
}
