use crate::{protocol, Snowflake};
use chrono::{DateTime, Utc};
use either::Either;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::ops::Deref;

pub type ChannelType = protocol::ChannelType;

// Separate type for each id so we can't confuse them
// Just a thin wrapper around a Snowflake with appropriate trait impls
macro_rules! id_type {
	($t:ident) => {
		#[derive(
			Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
		)]
		pub struct $t(Snowflake);

		impl fmt::Display for $t {
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				fmt::Display::fmt(&self.0, f)
			}
		}

		impl From<Snowflake> for $t {
			fn from(id: Snowflake) -> Self {
				$t(id)
			}
		}

		impl From<&Snowflake> for $t {
			fn from(id: &Snowflake) -> Self {
				$t(*id)
			}
		}

		impl From<u64> for $t {
			fn from(id: u64) -> Self {
				$t(Snowflake::from(id))
			}
		}

		impl PartialEq<u64> for $t {
			fn eq(&self, other: &u64) -> bool {
				PartialEq::eq(&self.0, other)
			}
		}

		impl Deref for $t {
			type Target = Snowflake;

			fn deref(&self) -> &Self::Target {
				&self.0
			}
		}
	};
}

id_type!(UserId);
id_type!(ChannelId);
id_type!(RoleId);
id_type!(MessageId);

pub trait Updatable<T> {
	fn update(&mut self, src: T);
}

pub trait CanUpdate<T> {
	fn update_dest(self, dest: &mut T);
}

impl<T> Updatable<T> for T {
	fn update(&mut self, src: T) {
		std::mem::replace(self, src);
	}
}

impl<T, U> CanUpdate<U> for T
where
	U: Updatable<T>,
{
	fn update_dest(self, dest: &mut U) {
		dest.update(self)
	}
}

pub trait HasId {
	type Id;
	fn id(&self) -> Self::Id;
}

#[derive(Debug)]
pub enum Event {
	GuildOnline,
	GuildOffline,
	MessageCreate(Message),
	MessageDelete(MessageId, ChannelId),
	MessageDeleteBulk(HashSet<MessageId>, ChannelId),
	MessageUpdate(Message),
	MemberJoin(UserId),
	MemberLeave(Either<Member, User>),
	MemberChangeNickname(UserId, Option<String>),
	MemberChangeUsername(UserId, String, String),
	MemberChangeAvatar(UserId, Option<String>),
	MemberChangePremium(UserId, Option<DateTime<Utc>>),
	MemberAddRole(UserId, RoleId),
	MemberRemoveRole(UserId, RoleId),
	MemberUpdate(protocol::GuildMemberUpdate),
	RoleCreate(RoleId),
	RoleDelete(RoleId, Option<Role>),
	RoleUpdate(RoleId, Option<Role>),
	ChannelCreate(ChannelId),
	ChannelDelete(ChannelId, Option<Channel>),
	ChannelUpdate(ChannelId, Option<Channel>),
	ReactionAdd(UserId, ChannelId, MessageId, ReactionEmoji),
	ReactionRemove(ChannelId, MessageId, ReactionRemoveType),
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
	Single(UserId, ReactionEmoji),
	Emoji(ReactionEmoji),
}

#[derive(Debug)]
pub struct Message {
	pub(crate) id: MessageId,
	pub(crate) channel_id: ChannelId,
	pub(crate) user_id: UserId,
	pub(crate) content: String,
	pub(crate) timestamp: DateTime<Utc>,
	pub(crate) edited_timestamp: Option<DateTime<Utc>>,
	pub(crate) tts: bool,
	pub(crate) mention_everyone: bool,
	pub(crate) mentions: HashSet<UserId>,
	pub(crate) mention_channels: HashSet<ChannelId>,
	pub(crate) pinned: bool,
}

impl Message {
	pub fn id(&self) -> MessageId {
		self.id
	}

	pub fn channel_id(&self) -> ChannelId {
		self.channel_id
	}

	pub fn user_id(&self) -> UserId {
		self.user_id
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

	pub fn mentions(&self) -> &HashSet<UserId> {
		&self.mentions
	}

	pub fn mention_channels(&self) -> &HashSet<ChannelId> {
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

impl From<&Message> for MessageId {
	fn from(m: &Message) -> Self {
		m.id
	}
}

impl From<&Message> for ChannelId {
	fn from(m: &Message) -> Self {
		m.channel_id
	}
}

impl From<&Message> for (ChannelId, MessageId) {
	fn from(m: &Message) -> Self {
		(m.channel_id.into(), m.id.into())
	}
}

impl From<&Message> for UserId {
	fn from(m: &Message) -> Self {
		m.user_id
	}
}

#[derive(Debug)]
pub struct User {
	pub(crate) id: UserId,
	pub(crate) username: String,
	pub(crate) discriminator: String,
	pub(crate) avatar: Option<String>,
	pub(crate) bot: bool,
	pub(crate) system: bool,
}

impl User {
	pub fn id(&self) -> UserId {
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

	pub fn is_bot(&self) -> bool {
		self.bot
	}

	pub fn is_system(&self) -> bool {
		self.system
	}
}

impl fmt::Display for User {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}#{}", self.username, self.discriminator)
	}
}

impl From<&User> for UserId {
	fn from(u: &User) -> Self {
		u.id
	}
}

#[derive(Debug)]
pub struct Member {
	pub(crate) user: User,
	pub(crate) nickname: Option<String>,
	pub(crate) roles: HashSet<RoleId>,
	pub(crate) hoisted_role: Option<RoleId>,
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

	pub fn roles(&self) -> &HashSet<RoleId> {
		&self.roles
	}

	pub fn hoisted_role(&self) -> Option<RoleId> {
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

	pub fn mention(&self) -> String {
		format!("<@!{}>", self.user.id)
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

impl From<&Member> for UserId {
	fn from(m: &Member) -> Self {
		m.user.id
	}
}

#[derive(Debug)]
pub struct Role {
	pub(crate) id: RoleId,
	pub(crate) name: String,
	pub(crate) color: u32,
	pub(crate) hoist: bool,
	pub(crate) position: u16,
	pub(crate) permissions: u64,
	pub(crate) managed: bool,
	pub(crate) mentionable: bool,
}

impl Role {
	pub fn id(&self) -> RoleId {
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

impl fmt::Display for Role {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		fmt::Display::fmt(&self.name, f)
	}
}

impl From<&Role> for RoleId {
	fn from(r: &Role) -> Self {
		r.id
	}
}

#[derive(Debug)]
pub struct Channel {
	pub(crate) id: ChannelId,
	pub(crate) channel_type: ChannelType,
	pub(crate) position: Option<u16>,
	pub(crate) name: String,
	pub(crate) topic: Option<String>,
	pub(crate) nsfw: Option<bool>,
	pub(crate) bitrate: Option<u32>,
	pub(crate) user_limit: Option<u16>,
	pub(crate) rate_limit_per_user: Option<u16>,
	pub(crate) parent_id: Option<ChannelId>,
}

impl Channel {
	pub fn id(&self) -> ChannelId {
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

	pub fn parent_id(&self) -> Option<ChannelId> {
		self.parent_id
	}
}

impl fmt::Display for Channel {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		fmt::Display::fmt(&self.name, f)
	}
}

impl HasId for Channel {
	type Id = ChannelId;
	fn id(&self) -> Self::Id {
		self.id
	}
}

impl From<&Channel> for ChannelId {
	fn from(c: &Channel) -> Self {
		c.id
	}
}
