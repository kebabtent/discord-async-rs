use crate::{protocol, Snowflake};
use chrono::{DateTime, Utc};
use either::Either;
use std::collections::HashSet;
use std::fmt;

pub type ChannelType = protocol::ChannelType;

#[derive(Debug)]
pub enum Event {
	GuildOnline,
	GuildOffline,
	MessageCreate(Message),
	MessageDelete {
		id: Snowflake,
		channel_id: Snowflake,
	},
	MessageDeleteBulk {
		ids: HashSet<Snowflake>,
		channel_id: Snowflake,
	},
	MessageUpdate(Message),
	MemberJoin(Snowflake),
	MemberLeave(Either<Member, User>),
	MemberChangeNickname {
		id: Snowflake,
		old_nickname: Option<String>,
	},
	MemberChangeUsername {
		id: Snowflake,
		old_username: String,
		old_discriminator: String,
	},
	MemberChangeAvatar {
		id: Snowflake,
		old_avatar: Option<String>,
	},
	MemberChangePremium {
		id: Snowflake,
		old_premium_since: Option<DateTime<Utc>>,
	},
	MemberAddRole {
		user_id: Snowflake,
		role_id: Snowflake,
	},
	MemberRemoveRole {
		user_id: Snowflake,
		role_id: Snowflake,
	},
	MemberUpdate(protocol::GuildMemberUpdate),
	RoleCreate(Snowflake),
	RoleDelete(Snowflake, Option<Role>),
	RoleUpdate(Snowflake, Option<Role>),
	ChannelCreate(Snowflake),
	ChannelDelete(Snowflake, Option<Channel>),
	ChannelUpdate(Snowflake, Option<Channel>),
	ReactionAdd {
		user_id: Snowflake,
		channel_id: Snowflake,
		message_id: Snowflake,
		emoji: ReactionEmoji,
	},
	ReactionRemove {
		channel_id: Snowflake,
		message_id: Snowflake,
		remove_type: ReactionRemoveType,
	},
	Other(protocol::Event),
}

#[derive(Debug)]
pub struct ReactionEmoji {
	pub id: Option<Snowflake>,
	pub name: Option<String>,
}

#[derive(Debug)]
pub enum ReactionRemoveType {
	All,
	Single {
		user_id: Snowflake,
		emoji: ReactionEmoji,
	},
	Emoji(ReactionEmoji),
}

#[derive(Debug)]
pub struct Message {
	pub(crate) id: Snowflake,
	pub(crate) channel_id: Snowflake,
	pub(crate) member_id: Snowflake,
	pub(crate) content: String,
	pub(crate) timestamp: DateTime<Utc>,
	pub(crate) edited_timestamp: Option<DateTime<Utc>>,
	pub(crate) tts: bool,
	pub(crate) mention_everyone: bool,
	pub(crate) mentions: HashSet<Snowflake>,
	pub(crate) mention_channels: HashSet<Snowflake>,
	pub(crate) pinned: bool,
}

impl Message {
	pub fn id(&self) -> Snowflake {
		self.id
	}

	pub fn channel_id(&self) -> Snowflake {
		self.channel_id
	}

	pub fn member_id(&self) -> Snowflake {
		self.member_id
	}

	pub fn content(&self) -> &str {
		&self.content
	}

	pub fn timestamp(&self) -> &DateTime<Utc> {
		&self.timestamp
	}

	pub fn edited_timestamp(&self) -> Option<&DateTime<Utc>> {
		self.edited_timestamp.as_ref()
	}

	pub fn is_tts(&self) -> bool {
		self.tts
	}

	pub fn is_mention_everyone(&self) -> bool {
		self.mention_everyone
	}

	pub fn mentions(&self) -> &HashSet<Snowflake> {
		&self.mentions
	}

	pub fn mention_channels(&self) -> &HashSet<Snowflake> {
		&self.mention_channels
	}

	pub fn is_pinned(&self) -> bool {
		self.pinned
	}
}

impl fmt::Display for Message {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		fmt::Display::fmt(&self.content, f)
	}
}

#[derive(Debug)]
pub struct User {
	pub(crate) id: Snowflake,
	pub(crate) username: String,
	pub(crate) discriminator: String,
	pub(crate) avatar: Option<String>,
}

impl User {
	pub fn id(&self) -> Snowflake {
		self.id
	}

	pub fn username(&self) -> &str {
		&self.username
	}

	pub fn discriminator(&self) -> &str {
		&self.discriminator
	}

	pub fn avatar(&self) -> Option<&str> {
		self.avatar.as_deref()
	}
}

impl fmt::Display for User {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}#{}", self.username, self.discriminator)
	}
}

#[derive(Debug)]
pub struct Member {
	pub(crate) user: User,
	pub(crate) nickname: Option<String>,
	pub(crate) roles: HashSet<Snowflake>,
	pub(crate) hoisted_role: Option<Snowflake>,
	pub(crate) joined_at: DateTime<Utc>,
	pub(crate) premium_since: Option<DateTime<Utc>>,
	pub(crate) mute: bool,
	pub(crate) deaf: bool,
}

impl Member {
	pub fn user(&self) -> &User {
		&self.user
	}

	pub fn nickname(&self) -> Option<&str> {
		self.nickname.as_deref()
	}

	pub fn roles(&self) -> &HashSet<Snowflake> {
		&self.roles
	}

	pub fn hoisted_role(&self) -> Option<Snowflake> {
		self.hoisted_role
	}

	pub fn joined_at(&self) -> &DateTime<Utc> {
		&self.joined_at
	}

	pub fn premium_since(&self) -> Option<&DateTime<Utc>> {
		self.premium_since.as_ref()
	}

	pub fn is_muted(&self) -> bool {
		self.mute
	}

	pub fn is_deafened(&self) -> bool {
		self.deaf
	}
}

impl fmt::Display for Member {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match &self.nickname {
			Some(n) => fmt::Display::fmt(n, f),
			None => fmt::Display::fmt(&self.user.username, f),
		}
	}
}

#[derive(Debug)]
pub struct Role {
	pub(crate) id: Snowflake,
	pub(crate) name: String,
	pub(crate) color: u32,
	pub(crate) hoist: bool,
	pub(crate) position: u16,
	pub(crate) permissions: u64,
	pub(crate) managed: bool,
	pub(crate) mentionable: bool,
}

impl Role {
	pub fn id(&self) -> Snowflake {
		self.id
	}

	pub fn name(&self) -> &str {
		&self.name
	}

	pub fn color(&self) -> u32 {
		self.color
	}

	pub fn is_hoisted(&self) -> bool {
		self.hoist
	}

	pub fn position(&self) -> u16 {
		self.position
	}

	pub fn permissions(&self) -> u64 {
		self.permissions
	}

	pub fn is_managed(&self) -> bool {
		self.managed
	}

	pub fn is_mentionable(&self) -> bool {
		self.mentionable
	}
}

#[derive(Debug)]
pub struct Channel {
	pub(crate) id: Snowflake,
	pub(crate) channel_type: ChannelType,
	pub(crate) position: Option<u16>,
	pub(crate) name: String,
	pub(crate) topic: Option<String>,
	pub(crate) nsfw: Option<bool>,
	pub(crate) bitrate: Option<u32>,
	pub(crate) user_limit: Option<u16>,
	pub(crate) rate_limit_per_user: Option<u16>,
	pub(crate) parent_id: Option<Snowflake>,
}

impl Channel {
	pub fn id(&self) -> Snowflake {
		self.id
	}

	pub fn channel_type(&self) -> ChannelType {
		self.channel_type
	}

	pub fn position(&self) -> Option<u16> {
		self.position
	}

	pub fn name(&self) -> &str {
		&self.name
	}

	pub fn topic(&self) -> Option<&str> {
		self.topic.as_deref()
	}

	pub fn is_nsfw(&self) -> bool {
		self.nsfw.unwrap_or(false)
	}

	pub fn bitrate(&self) -> Option<u32> {
		self.bitrate
	}

	pub fn user_limit(&self) -> Option<u16> {
		self.user_limit
	}

	pub fn rate_limit_per_user(&self) -> Option<u16> {
		self.rate_limit_per_user
	}

	pub fn parent_id(&self) -> Option<Snowflake> {
		self.parent_id
	}
}

impl fmt::Display for Channel {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		fmt::Display::fmt(&self.name, f)
	}
}
