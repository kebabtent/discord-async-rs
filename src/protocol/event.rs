use super::ProtocolError;
use crate::types;
use crate::Snowflake;
use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashSet;
use std::convert::{TryFrom, TryInto};
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
								(9, _) => Event::InvalidSession(map.next_value()?),
								(10, _) => Event::Hello(map.next_value()?),
								(11, _) => {
									map.next_value::<IgnoredAny>()?;
									Event::HearbeatAck
								}
								(0, Some(t)) => match t {
									"READY" => Event::Ready(map.next_value()?),
									"RESUMED" => {
										map.next_value::<IgnoredAny>()?;
										Event::Resumed
									}
									"GUILD_CREATE" => Event::GuildCreate(map.next_value()?),
									"MESSAGE_CREATE" => Event::MessageCreate(map.next_value()?),
									"MESSAGE_DELETE" => Event::MessageDelete(map.next_value()?),
									"MESSAGE_DELETE_BULK" => {
										Event::MessageDeleteBulk(map.next_value()?)
									}
									"MESSAGE_UPDATE" => Event::MessageUpdate(map.next_value()?),
									"GUILD_MEMBER_ADD" => Event::GuildMemberAdd(map.next_value()?),
									"GUILD_MEMBER_REMOVE" => {
										Event::GuildMemberRemove(map.next_value()?)
									}
									"GUILD_MEMBER_UPDATE" => {
										Event::GuildMemberUpdate(map.next_value()?)
									}
									"GUILD_ROLE_CREATE" => {
										Event::GuildRoleCreate(map.next_value()?)
									}
									"GUILD_ROLE_DELETE" => {
										Event::GuildRoleDelete(map.next_value()?)
									}
									"GUILD_ROLE_UPDATE" => {
										Event::GuildRoleUpdate(map.next_value()?)
									}
									"CHANNEL_CREATE" => Event::ChannelCreate(map.next_value()?),
									"CHANNEL_DELETE" => Event::ChannelDelete(map.next_value()?),
									"CHANNEL_UPDATE" => Event::ChannelUpdate(map.next_value()?),
									"MESSAGE_REACTION_ADD" => {
										Event::MessageReactionAdd(map.next_value()?)
									}
									"MESSAGE_REACTION_REMOVE" => {
										Event::MessageReactionRemove(map.next_value()?)
									}
									"MESSAGE_REACTION_REMOVE_ALL" => {
										Event::MessageReactionRemoveAll(map.next_value()?)
									}
									"MESSAGE_REACTION_REMOVE_EMOJI" => {
										Event::MessageReactionRemoveEmoji(map.next_value()?)
									}
									t => {
										map.next_value::<IgnoredAny>()?;
										Event::Unknown(t.into())
									}
								},
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
	InvalidSession(InvalidSession),
	HearbeatAck,
	GuildCreate(GuildCreate),
	MessageCreate(MessageCreate),
	MessageDelete(MessageDelete),
	MessageDeleteBulk(MessageDeleteBulk),
	MessageUpdate(MessageUpdate),
	GuildMemberAdd(GuildMemberAdd),
	GuildMemberRemove(GuildMemberRemove),
	GuildMemberUpdate(GuildMemberUpdate),
	GuildRoleCreate(GuildRoleCreate),
	GuildRoleDelete(GuildRoleDelete),
	GuildRoleUpdate(GuildRoleUpdate),
	ChannelCreate(ChannelCreate),
	ChannelDelete(ChannelDelete),
	ChannelUpdate(ChannelUpdate),
	MessageReactionAdd(MessageReactionAdd),
	MessageReactionRemove(MessageReactionRemove),
	MessageReactionRemoveAll(MessageReactionRemoveAll),
	MessageReactionRemoveEmoji(MessageReactionRemoveEmoji),
	Unknown(String),
}

impl Event {
	pub fn guild_id(&self) -> Option<Snowflake> {
		match self {
			Event::MessageCreate(e) => e.message.guild_id,
			Event::MessageDelete(e) => e.guild_id,
			Event::MessageDeleteBulk(e) => e.guild_id,
			Event::MessageUpdate(e) => e.message.guild_id,
			Event::GuildMemberAdd(e) => Some(e.guild_id),
			Event::GuildMemberRemove(e) => Some(e.guild_id),
			Event::GuildMemberUpdate(e) => Some(e.guild_id),
			Event::GuildRoleCreate(e) => Some(e.guild_id),
			Event::GuildRoleDelete(e) => Some(e.guild_id),
			Event::GuildRoleUpdate(e) => Some(e.guild_id),
			Event::ChannelCreate(e) => e.channel.guild_id,
			Event::ChannelDelete(e) => e.channel.guild_id,
			Event::ChannelUpdate(e) => e.channel.guild_id,
			Event::MessageReactionAdd(e) => e.guild_id,
			Event::MessageReactionRemove(e) => e.guild_id,
			Event::MessageReactionRemoveAll(e) => e.guild_id,
			Event::MessageReactionRemoveEmoji(e) => e.guild_id,
			_ => None,
		}
	}

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

	pub fn is_heartbeat_ack(&self) -> bool {
		match self {
			Event::HearbeatAck => true,
			_ => false,
		}
	}
}

impl fmt::Display for Event {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Event::Hello(_) => write!(f, "Hello"),
			Event::Ready(e) => write!(f, "Ready(username={})", e.user.username),
			Event::Resumed => write!(f, "Resumed"),
			Event::InvalidSession(_) => write!(f, "InvalidSession"),
			Event::HearbeatAck => write!(f, "HeartbeatAck"),
			Event::GuildCreate(e) => write!(f, "GuildCreate(id={}, name={})", e.id, e.name),
			Event::MessageCreate(e) => write!(
				f,
				"MessageCreate(user={}, channel_id={})",
				e.message.author, e.message.channel_id
			),
			Event::MessageDelete(e) => {
				write!(f, "MessageDelete(channel_id={}, id={})", e.channel_id, e.id)
			}
			Event::MessageDeleteBulk(e) => write!(
				f,
				"MessageDeleteBulk(channel_id={}, count={})",
				e.channel_id,
				e.ids.len()
			),
			Event::MessageUpdate(e) => write!(
				f,
				"MessageUpdate(user={}, channel_id={})",
				e.message.author, e.message.channel_id
			),
			Event::GuildMemberAdd(e) => write!(
				f,
				"GuildMemberAdd(guild_id={}, user={})",
				e.guild_id, e.member
			),
			Event::GuildMemberRemove(e) => write!(
				f,
				"GuildMemberRemove(guild={}, user={})",
				e.guild_id, e.user
			),
			Event::GuildMemberUpdate(e) => write!(
				f,
				"GuildMemberUpdate(guild={}, user={})",
				e.guild_id, e.user
			),
			Event::GuildRoleCreate(e) => write!(
				f,
				"GuildRoleCreate(guild={}, role={})",
				e.guild_id, e.role.id
			),
			Event::GuildRoleDelete(e) => write!(
				f,
				"GuildRoleDelete(guild={}, role={})",
				e.guild_id, e.role_id
			),
			Event::GuildRoleUpdate(e) => write!(
				f,
				"GuildRoleUpdate(guild={}, role={})",
				e.guild_id, e.role.id
			),
			Event::ChannelCreate(e) => write!(f, "ChannelCreate(id={})", e.channel.id),
			Event::ChannelDelete(e) => write!(f, "ChannelDelete(id={})", e.channel.id),
			Event::ChannelUpdate(e) => write!(f, "ChannelUpdate(id={})", e.channel.id),
			Event::MessageReactionAdd(e) => write!(
				f,
				"MessageReactionAdd(message={}, emoji={})",
				e.message_id, e.emoji
			),
			Event::MessageReactionRemove(e) => write!(
				f,
				"MessageReactionRemove(message={}, emoji={})",
				e.message_id, e.emoji
			),
			Event::MessageReactionRemoveAll(e) => {
				write!(f, "MessageReactionRemoveAll(message={})", e.message_id)
			}
			Event::MessageReactionRemoveEmoji(e) => write!(
				f,
				"MessageReactionRemoveEmoji(message={}, emoji={})",
				e.message_id, e.emoji
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

impl From<MessageCreate> for Event {
	fn from(event: MessageCreate) -> Self {
		Event::MessageCreate(event)
	}
}

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

#[derive(Clone, Debug, Deserialize)]
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
	pub flags: Option<super::UserFlags>,
	#[serde(default)]
	pub premium_type: Option<u64>,
	#[serde(default)]
	pub public_flags: Option<super::UserFlags>,
}

impl fmt::Display for User {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}#{}", self.username, self.discriminator)
	}
}

impl TryFrom<User> for types::User {
	type Error = ProtocolError;

	fn try_from(u: User) -> Result<Self, Self::Error> {
		Ok(Self {
			id: u.id,
			username: u.username,
			discriminator: u.discriminator,
			avatar: u.avatar,
		})
	}
}

#[derive(Debug, Deserialize)]
pub struct Member {
	pub user: User,
	#[serde(default)]
	pub nick: Option<String>,
	pub roles: Vec<Snowflake>,
	pub joined_at: String,
	#[serde(default)]
	pub hoisted_role: Option<Snowflake>,
	#[serde(default)]
	pub premium_since: Option<String>,
	pub deaf: bool,
	pub mute: bool,
}

impl fmt::Display for Member {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match &self.nick {
			Some(n) => fmt::Display::fmt(n, f)?,
			None => write!(f, "??")?,
		}
		write!(f, " ({})", self.user)
	}
}

impl TryFrom<Member> for types::Member {
	type Error = ProtocolError;

	fn try_from(m: Member) -> Result<Self, Self::Error> {
		Ok(Self {
			user: m.user.try_into()?,
			nickname: m.nick,
			roles: m.roles.into_iter().collect(),
			hoisted_role: m.hoisted_role,
			joined_at: m
				.joined_at
				.parse()
				.map_err(|_| ProtocolError::InvalidField("member join date"))?,
			premium_since: m
				.premium_since
				.map(|p| p.parse())
				.transpose()
				.map_err(|_| ProtocolError::InvalidField("member premium date"))?,
			mute: m.mute,
			deaf: m.deaf,
		})
	}
}

#[derive(Debug, Deserialize)]
pub struct MessageMember {
	pub nick: Option<String>,
	pub roles: Vec<Snowflake>,
	pub joined_at: String,
	#[serde(default)]
	pub hoisted_role: Option<Snowflake>,
	#[serde(default)]
	pub premium_since: Option<String>,
	pub deaf: bool,
	pub mute: bool,
}

impl TryFrom<(MessageMember, User)> for types::Member {
	type Error = ProtocolError;

	fn try_from(from: (MessageMember, User)) -> Result<Self, Self::Error> {
		let (m, user) = from;
		TryFrom::try_from(Member {
			user,
			nick: m.nick,
			roles: m.roles,
			joined_at: m.joined_at,
			hoisted_role: m.hoisted_role,
			premium_since: m.premium_since,
			deaf: m.deaf,
			mute: m.mute,
		})
	}
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

impl From<Role> for types::Role {
	fn from(r: Role) -> Self {
		Self {
			id: r.id,
			name: r.name,
			color: r.color,
			hoist: r.hoist,
			position: r.position,
			permissions: r.permissions,
			managed: r.managed,
			mentionable: r.mentionable,
		}
	}
}

#[derive(Debug, Deserialize)]
pub struct Message {
	pub id: Snowflake,
	pub channel_id: Snowflake,
	#[serde(default)]
	pub guild_id: Option<Snowflake>,
	pub author: User,
	#[serde(default)]
	pub member: Option<MessageMember>,
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
	pub message_type: super::MessageType,
	// activity
	// application
	// message_reference
	// flags
}

impl TryFrom<(Message, Snowflake)> for types::Message {
	type Error = ProtocolError;

	fn try_from(m: (Message, Snowflake)) -> Result<Self, Self::Error> {
		let (m, member_id) = m;
		Ok(Self {
			id: m.id,
			channel_id: m.channel_id,
			member_id,
			content: m.content,
			timestamp: m
				.timestamp
				.parse()
				.map_err(|_| ProtocolError::InvalidField("message timestamp"))?,
			edited_timestamp: m
				.edited_timestamp
				.map(|t| t.parse())
				.transpose()
				.map_err(|_| ProtocolError::InvalidField("message edited timestamp"))?,
			tts: m.tts,
			mention_everyone: m.mention_everyone,
			mentions: m.mentions.into_iter().map(|m| m.id).collect(),
			mention_channels: m.mention_channels.into_iter().map(|m| m.id).collect(),
			pinned: m.pinned,
		})
	}
}

#[derive(Debug, Deserialize)]
pub struct Emoji {
	pub id: Snowflake,
	pub name: String,
	#[serde(default)]
	pub roles: Option<Vec<Snowflake>>,
	#[serde(default)]
	pub user: Option<User>,
	pub require_colons: bool,
	pub managed: bool,
	pub animated: bool,
	pub available: bool,
}

#[derive(Debug, Deserialize)]
pub struct ReactionEmoji {
	#[serde(default)]
	pub id: Option<Snowflake>,
	#[serde(default)]
	pub name: Option<String>,
	#[serde(default)]
	pub animated: bool,
}

impl fmt::Display for ReactionEmoji {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		if let Some(n) = &self.name {
			fmt::Display::fmt(n, f)
		} else if let Some(i) = &self.id {
			fmt::Display::fmt(i, f)
		} else {
			write!(f, "??")
		}
	}
}

impl TryFrom<ReactionEmoji> for types::ReactionEmoji {
	type Error = ProtocolError;

	fn try_from(e: ReactionEmoji) -> Result<Self, Self::Error> {
		// Either name or ID (or both) needs to be set
		if e.id.is_none() && e.name.is_none() {
			return Err(ProtocolError::MissingField("reaction emoji id or name"));
		}

		Ok(Self {
			id: e.id,
			name: e.name,
		})
	}
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
	#[serde(rename = "type")]
	pub channel_type: super::ChannelType,
	#[serde(default)]
	pub guild_id: Option<Snowflake>,
	#[serde(default)]
	pub position: Option<u16>,
	//permission_overwrites
	#[serde(default)]
	pub name: Option<String>,
	#[serde(default)]
	pub topic: Option<String>,
	#[serde(default)]
	pub nsfw: Option<bool>,
	// last_message_id
	#[serde(default)]
	pub bitrate: Option<u32>,
	#[serde(default)]
	pub user_limit: Option<u16>,
	#[serde(default)]
	pub rate_limit_per_user: Option<u16>,
	// recipients
	// #[serde(default)]
	// pub icon: Option<String>,
	// #[serde(default)]
	// pub owner_id: Option<Snowflake>,
	// #[serde(default)]
	// pub application_id: Option<Snowflake>,
	#[serde(default)]
	pub parent_id: Option<Snowflake>,
	// last_pin_timestamp
}

impl TryFrom<Channel> for types::Channel {
	type Error = ProtocolError;

	fn try_from(c: Channel) -> Result<Self, Self::Error> {
		Ok(Self {
			id: c.id,
			channel_type: c.channel_type,
			position: c.position,
			name: c
				.name
				.ok_or_else(|| ProtocolError::MissingField("channel name"))?,
			topic: c.topic.filter(|t| !t.is_empty()),
			nsfw: c.nsfw,
			bitrate: c.bitrate,
			user_limit: c.user_limit,
			rate_limit_per_user: c.rate_limit_per_user.filter(|l| *l > 0),
			parent_id: c.parent_id,
		})
	}
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
	pub members: Vec<Member>,
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

impl From<GuildCreate> for Event {
	fn from(gc: GuildCreate) -> Self {
		Event::GuildCreate(gc)
	}
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct MessageCreate {
	pub message: Message,
}

#[derive(Debug, Deserialize)]
pub struct MessageDelete {
	pub id: Snowflake,
	pub channel_id: Snowflake,
	#[serde(default)]
	pub guild_id: Option<Snowflake>,
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct MessageUpdate {
	pub message: Message,
}

#[derive(Debug, Deserialize)]
pub struct MessageDeleteBulk {
	pub ids: HashSet<Snowflake>,
	pub channel_id: Snowflake,
	#[serde(default)]
	pub guild_id: Option<Snowflake>,
}

#[derive(Debug, Deserialize)]
pub struct GuildMemberAdd {
	pub guild_id: Snowflake,
	#[serde(flatten)]
	pub member: Member,
}

#[derive(Debug, Deserialize)]
pub struct GuildMemberRemove {
	pub guild_id: Snowflake,
	pub user: User,
}

#[derive(Debug, Deserialize)]
pub struct GuildMemberUpdate {
	pub guild_id: Snowflake,
	pub roles: HashSet<Snowflake>,
	pub user: User,
	#[serde(default)]
	pub nick: Option<String>,
	#[serde(default)]
	pub premium_since: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GuildRoleCreate {
	pub guild_id: Snowflake,
	pub role: Role,
}

#[derive(Debug, Deserialize)]
pub struct GuildRoleDelete {
	pub guild_id: Snowflake,
	pub role_id: Snowflake,
}

#[derive(Debug, Deserialize)]
pub struct GuildRoleUpdate {
	pub guild_id: Snowflake,
	pub role: Role,
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct ChannelCreate {
	pub channel: Channel,
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct ChannelDelete {
	pub channel: Channel,
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct ChannelUpdate {
	pub channel: Channel,
}

#[derive(Debug, Deserialize)]
pub struct MessageReactionAdd {
	pub user_id: Snowflake,
	pub channel_id: Snowflake,
	pub message_id: Snowflake,
	#[serde(default)]
	pub guild_id: Option<Snowflake>,
	#[serde(default)]
	pub member: Option<Member>,
	pub emoji: ReactionEmoji,
}

#[derive(Debug, Deserialize)]
pub struct MessageReactionRemove {
	pub user_id: Snowflake,
	pub channel_id: Snowflake,
	pub message_id: Snowflake,
	#[serde(default)]
	pub guild_id: Option<Snowflake>,
	pub emoji: ReactionEmoji,
}

#[derive(Debug, Deserialize)]
pub struct MessageReactionRemoveAll {
	pub channel_id: Snowflake,
	pub message_id: Snowflake,
	#[serde(default)]
	pub guild_id: Option<Snowflake>,
}

#[derive(Debug, Deserialize)]
pub struct MessageReactionRemoveEmoji {
	pub channel_id: Snowflake,
	pub message_id: Snowflake,
	#[serde(default)]
	pub guild_id: Option<Snowflake>,
	pub emoji: ReactionEmoji,
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct InvalidSession {
	pub resumable: bool,
}
