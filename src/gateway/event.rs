use crate::protocol::event::*;
use crate::Snowflake;
use serde::de;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt;

#[derive(Debug)]
pub struct EventError;

#[derive(Debug)]
pub struct Payload {
	pub sequence: Option<u64>,
	pub event: Event,
}

impl fmt::Display for Payload {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self.sequence {
			Some(seq) => write!(f, "{}@{}", self.event, seq),
			None => write!(f, "{}", self.event),
		}
	}
}

impl<'de> Deserialize<'de> for Payload {
	fn deserialize<D>(deserializer: D) -> Result<Payload, D::Error>
	where
		D: Deserializer<'de>,
	{
		struct PayloadVisitor;

		impl<'de> Visitor<'de> for PayloadVisitor {
			type Value = Payload;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("struct Payload")
			}

			fn visit_map<V>(self, mut map: V) -> Result<Payload, V::Error>
			where
				V: MapAccess<'de>,
			{
				let mut t: Option<String> = None;
				let mut sequence = None;
				let mut op: Option<u8> = None;
				let mut payload = None;
				while let Some(key) = map.next_key()? {
					match key {
						"t" => {
							if t.is_some() {
								return Err(de::Error::duplicate_field("t"));
							}
							t = map.next_value()?;
						}
						"s" => {
							if sequence.is_some() {
								return Err(de::Error::duplicate_field("s"));
							}
							sequence = map.next_value()?;
						}
						"op" => {
							if op.is_some() {
								return Err(de::Error::duplicate_field("op"));
							}
							op = Some(map.next_value()?);
						}
						"d" => {
							if payload.is_some() {
								return Err(de::Error::duplicate_field("d"));
							}

							let op = match op {
								Some(op) => op,
								None => return Err(de::Error::missing_field("op")),
							};

							let event = match (op, t.as_deref()) {
								(10, _) => Event::Hello(map.next_value()?),
								(11, _) => {
									map.next_value::<IgnoredAny>()?;
									Event::HearbeatAck
								}
								(0, Some("READY")) => Event::Ready(map.next_value()?),
								(0, Some("RESUMED")) => {
									map.next_value::<IgnoredAny>()?;
									Event::Resumed
								}
								(0, Some("GUILD_CREATE")) => Event::GuildCreate(map.next_value()?),
								(0, Some("MESSAGE_CREATE")) => {
									Event::MessageCreate(map.next_value()?)
								}
								(0, Some(t)) => {
									map.next_value::<IgnoredAny>()?;
									Event::Unknown(t.into())
								}
								(0, None) => return Err(de::Error::missing_field("t")),
								(e, _) => {
									map.next_value::<IgnoredAny>()?;
									Event::Unknown(format!("{}", e))
								}
							};
							payload = Some(Payload { sequence, event });
						}
						_ => {
							map.next_value::<IgnoredAny>()?;
						}
					}
				}
				payload.ok_or_else(|| de::Error::missing_field("d"))
			}
		}

		deserializer.deserialize_struct("Payload", &["t", "s", "op", "d"], PayloadVisitor)
	}
}

#[derive(Debug, Deserialize)]
pub enum Event {
	Hello(Hello),
	Ready(Ready),
	Resumed,
	HearbeatAck,
	GuildCreate(GuildCreate),
	MessageCreate(MessageCreate),
	Unknown(String),
}

impl Event {
	pub fn expect_hello(self) -> Result<Hello, EventError> {
		match self {
			Event::Hello(event) => Ok(event),
			_ => Err(EventError),
		}
	}

	pub fn expect_ready(self) -> Result<Ready, EventError> {
		match self {
			Event::Ready(ready) => Ok(ready),
			_ => Err(EventError),
		}
	}

	pub fn expect_guild_create(self) -> Result<GuildCreate, EventError> {
		match self {
			Event::GuildCreate(guild_create) => Ok(guild_create),
			_ => Err(EventError),
		}
	}

	pub fn expect_unknown(self) -> Result<String, EventError> {
		match self {
			Event::Unknown(uknown) => Ok(uknown),
			_ => Err(EventError),
		}
	}
}

impl fmt::Display for Event {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Event::Hello(_) => write!(f, "Hello"),
			Event::Ready(e) => write!(f, "Ready(username={})", e.user.username),
			Event::Resumed => write!(f, "Resumed"),
			Event::HearbeatAck => write!(f, "HeartbeatAck"),
			Event::GuildCreate(e) => write!(f, "GuildCreate(id={}, name={})", e.id, e.name),
			Event::MessageCreate(e) => write!(
				f,
				"MessageCreate(user={}#{}, channel_id={})",
				e.author.username, e.author.discriminator, e.channel_id
			),
			Event::Unknown(n) => write!(f, "Unknown({})", n),
		}
	}
}

impl From<Hello> for Event {
	fn from(event: Hello) -> Self {
		Event::Hello(event)
	}
}

#[derive(Debug, Deserialize)]
pub struct Member {
	#[serde(default)]
	pub user: Option<User>,
	pub nick: Option<String>,
	pub roles: Vec<Snowflake>,
	pub joined_at: String,
	#[serde(default)]
	pub premium_since: Option<String>,
	pub deaf: bool,
	pub mute: bool,
}

#[derive(Debug, Deserialize)]
pub struct Role {
	pub id: Snowflake,
	pub name: String,
	pub color: u32,
	pub hoist: bool,
	pub position: u16,
	pub permissions: u64,
	pub managed: bool,
	pub mentionable: bool,
}

#[derive(Debug, Deserialize)]
pub struct Emoji {
	pub id: Snowflake,
	pub name: String,
	pub roles: Option<Vec<Snowflake>>,
	pub user: Option<User>,
}

#[derive(Debug, Deserialize)]
pub struct VoiceState {
	pub channel_id: Snowflake,
	pub user_id: Snowflake,
	pub session_id: String,
	pub deaf: bool,
	pub mute: bool,
	pub self_deaf: bool,
	pub self_mute: bool,
	pub self_stream: Option<bool>,
	pub suppress: bool,
}

#[derive(Debug, Deserialize)]
pub struct Channel {
	pub id: Snowflake,
	#[serde(default)]
	pub guild_id: Option<Snowflake>,
	#[serde(rename = "type")]
	pub channel_type: u8,
	pub position: Option<u16>,
	pub name: Option<String>,
	pub topic: Option<String>,
	// TODO
}

#[derive(Debug, Deserialize)]
pub struct GuildCreate {
	pub id: Snowflake,
	pub name: String,
	pub icon: Option<String>,
	pub splash: Option<String>,
	pub discovery_splash: Option<String>,
	#[serde(default)]
	pub owner: Option<bool>,
	pub owner_id: Snowflake,
	#[serde(default)]
	pub permissions: Option<u64>,
	pub region: String,
	pub afk_channel_id: Option<Snowflake>,
	pub afk_timeout: u64,
	pub verification_level: u8,
	pub default_message_notifications: u8,
	pub explicit_content_filter: u8,
	pub roles: Vec<Role>,
	pub emojis: Vec<Emoji>,
	pub features: Vec<String>,
	pub mfa_level: u8,
	#[serde(default)]
	pub widget_enabled: Option<bool>,
	#[serde(default)]
	pub widget_channel_id: Option<Snowflake>,
	pub system_channel_id: Option<Snowflake>,
	pub system_channel_flags: u8,
	pub rules_channel_id: Option<Snowflake>,
	pub joined_at: String,
	pub large: bool,
	pub unavailable: bool,
	pub member_count: u16,
	pub voice_states: Vec<VoiceState>,
	//members
	pub channels: Vec<Channel>,
	//presences
	//max_presences
	pub max_members: Option<u16>,
	pub vanity_url_code: Option<String>,
	pub description: Option<String>,
	pub banner: Option<String>,
	pub premium_tier: u8,
	#[serde(default)]
	pub premium_subscription_count: Option<u16>,
	pub preferred_locale: String,
	pub public_updates_channel_id: Option<Snowflake>,
	#[serde(default)]
	pub max_video_channel_users: Option<u16>,
	#[serde(default)]
	pub approximate_member_count: Option<u16>,
	#[serde(default)]
	pub approximate_presence_count: Option<u16>,
}

#[derive(Debug, Deserialize)]
pub struct Mention {}

#[derive(Debug, Deserialize)]
pub struct ChannelMention {
	pub id: Snowflake,
	pub guild_id: Snowflake,
	#[serde(rename = "type")]
	pub channel_type: u8,
	pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct MessageCreate {
	pub id: Snowflake,
	pub channel_id: Snowflake,
	pub guild_id: Option<Snowflake>,
	pub author: User,
	#[serde(default)]
	pub member: Option<Member>,
	pub content: String,
	pub timestamp: String,
	pub edited_timestamp: Option<String>,
	pub tts: bool,
	pub mention_everyone: bool,
	pub mentions: Vec<User>,
	#[serde(default)]
	pub mention_channels: Vec<ChannelMention>,
	// attachments
	// embeds
	// reactions
	// nonce
	pub pinned: bool,
	#[serde(default)]
	pub webhook_id: Option<Snowflake>,
	#[serde(rename = "type")]
	pub message_type: u8,
	// activity
	// application
	// message_reference
	// flags
}
