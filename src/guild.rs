use crate::protocol::{GuildCreate, MessageType, ProtocolError};
use crate::{protocol, types::*};
use crate::{Client, Recv, Snowflake};
use either::Either;
use futures::StreamExt;
use log::{info, warn};
use std::collections::HashMap;
use std::convert::TryInto;
use std::mem;

// Macro to log and skip fallible operations
macro_rules! t {
	($expr:expr) => {
		match $expr {
			Ok(val) => val,
			Err(e) => {
				warn!("{:?}", e);
				continue;
				}
			}
	};
}

// Macro to try to convert one type to another
macro_rules! ti {
	($expr:expr) => {
		t!($expr.try_into())
	};
}

pub struct GuildSeed {
	guild_create: protocol::GuildCreate,
	client: Client,
	event_recv: Recv<GuildEvent>,
}

impl GuildSeed {
	pub(crate) fn new(
		guild_create: protocol::GuildCreate,
		client: Client,
		event_recv: Recv<GuildEvent>,
	) -> Self {
		Self {
			guild_create,
			client,
			event_recv,
		}
	}

	pub fn id(&self) -> Snowflake {
		self.guild_create.id
	}

	pub fn name(&self) -> &str {
		&self.guild_create.name
	}

	fn inner(self) -> (protocol::GuildCreate, Client, Recv<GuildEvent>) {
		(self.guild_create, self.client, self.event_recv)
	}
}

pub struct Guild {
	id: Snowflake,
	name: String,
	available: bool,
	channels: HashMap<ChannelId, Channel>,
	roles: HashMap<RoleId, Role>,
	members: HashMap<UserId, Member>,
	recv: Recv<GuildEvent>,
	client: Client,
}

impl Guild {
	pub fn new(seed: GuildSeed) -> Result<Self, ProtocolError> {
		let (gc, client, recv) = seed.inner();

		let mut guild = Self {
			id: gc.id,
			name: String::new(),
			available: false,
			channels: HashMap::new(),
			roles: HashMap::new(),
			members: HashMap::new(),
			recv,
			client,
		};
		// guild.update(gc)?;
		Self::update(&mut guild, gc)?;
		Ok(guild)
	}

	pub fn client(&self) -> Client {
		self.client.clone()
	}

	fn update(&mut self, gc: GuildCreate) -> Result<(), ProtocolError> {
		self.name = gc.name;

		for channel in gc.channels {
			self.insert_channel(channel.try_into()?);
		}

		for role in gc.roles {
			self.insert_role(role.into());
		}

		for member in gc.members {
			self.insert_member(member.try_into()?);
		}

		info!(
			"Loaded guild '{}' ({} channels, {} roles, {}/{} members)",
			self.name,
			self.channels.len(),
			self.roles.len(),
			self.members.len(),
			gc.member_count,
		);

		Ok(())
	}

	pub fn id(&self) -> Snowflake {
		self.id
	}

	pub fn name(&self) -> &str {
		&self.name
	}

	pub fn available(&self) -> bool {
		self.available
	}

	pub fn channels(&self) -> impl Iterator<Item = &Channel> {
		self.channels.values()
	}

	pub fn channel<T: Into<ChannelId>>(&self, id: T) -> Option<&Channel> {
		self.channels.get(&id.into())
	}

	fn insert_channel(&mut self, src: Channel) -> Option<Channel> {
		self.channels.insert(src.id, src)
	}

	fn delete_channel<T: Into<ChannelId>>(&mut self, id: T) -> Option<Channel> {
		self.channels.remove(&id.into())
	}

	pub fn roles(&self) -> impl Iterator<Item = &Role> {
		self.roles.values()
	}

	pub fn role<T: Into<RoleId>>(&self, id: T) -> Option<&Role> {
		self.roles.get(&id.into())
	}

	fn insert_role(&mut self, src: Role) -> Option<Role> {
		self.roles.insert(src.id, src)
	}

	fn delete_role<T: Into<RoleId>>(&mut self, id: T) -> Option<Role> {
		self.roles.remove(&id.into())
	}

	pub fn members(&self) -> impl Iterator<Item = &Member> {
		self.members.values()
	}

	pub fn member<T: Into<UserId>>(&self, id: T) -> Option<&Member> {
		self.members.get(&id.into())
	}

	fn insert_member(&mut self, src: Member) -> Option<Member> {
		self.members.insert(src.user.id, src)
	}

	fn delete_member<T: Into<UserId>>(&mut self, id: T) -> Option<Member> {
		self.members.remove(&id.into())
	}

	pub async fn next_stateless(&mut self) -> Option<GuildEvent> {
		self.recv.next().await
	}

	pub async fn next(&mut self) -> Option<Event> {
		// Some `GuildEvent`s might not result in an `Event`, so we loop until we get one
		loop {
			let event = match self.recv.next().await? {
				GuildEvent::Online => return Some(Event::GuildOnline),
				GuildEvent::Offline => return Some(Event::GuildOffline),
				GuildEvent::Event(e) => e,
			};

			use protocol::Event::*;
			let event = match event {
				MessageCreate(protocol::MessageCreate { message }) => {
					// Skip webhook messages
					if message.webhook_id.is_some() {
						continue;
					}

					// Skip messages without a member
					let member_id = message.author.id;
					if message.member.is_none() {
						continue;
					}

					if message.message_type != MessageType::Default {
						continue;
					}

					Event::MessageCreate(ti!((message, member_id)))
				}
				MessageDelete(md) => Event::MessageDelete(md.id.into(), md.channel_id.into()),
				MessageDeleteBulk(md) => Event::MessageDeleteBulk(
					md.ids.iter().map(|m| m.into()).collect(),
					md.channel_id.into(),
				),
				MessageUpdate(protocol::MessageUpdate { message }) => {
					// Skip webhook messages
					if message.webhook_id.is_some() {
						continue;
					}

					// Skip messages without a member
					let member_id = message.author.id;
					if message.member.is_none() {
						continue;
					}

					if message.message_type != MessageType::Default {
						continue;
					}

					Event::MessageUpdate(ti!((message, member_id)))
				}
				GuildMemberAdd(ma) => {
					let id = ma.member.user.id.into();
					self.insert_member(ti!(ma.member));
					Event::MemberJoin(id)
				}
				GuildMemberRemove(mr) => {
					let member = match self.delete_member(mr.user.id) {
						Some(m) => Either::Left(m),
						None => Either::<_, User>::Right(ti!(mr.user)),
					};
					Event::MemberLeave(member)
				}
				GuildMemberUpdate(mu) => {
					let id = mu.user.id.into();

					// Figure out which field changed and fire appropriate event
					let premium_since =
						t!(mu.premium_since.as_ref().map(|s| s.parse()).transpose());
					if let Some(member) = self.members.get_mut(&id) {
						if mu.nick != member.nickname {
							Event::MemberChangeNickname(
								id,
								mem::replace(&mut member.nickname, mu.nick),
							)
						} else if mu.user.username != member.user.username
							|| mu.user.discriminator != member.user.discriminator
						{
							Event::MemberChangeUsername(
								id,
								mem::replace(&mut member.user.username, mu.user.username),
								mem::replace(&mut member.user.discriminator, mu.user.discriminator),
							)
						} else if mu.user.avatar != member.user.avatar {
							Event::MemberChangeAvatar(
								id,
								mem::replace(&mut member.user.avatar, mu.user.avatar),
							)
						} else if premium_since != member.premium_since {
							Event::MemberChangePremium(
								id,
								mem::replace(&mut member.premium_since, premium_since),
							)
						} else if let Some(role_id) = mu
							.roles()
							.symmetric_difference(&member.roles)
							.next()
							.map(|r| *r)
						{
							let event = if mu.roles.contains(&role_id) {
								Event::MemberAddRole(id, role_id)
							} else {
								Event::MemberRemoveRole(id, role_id)
							};
							mem::replace(&mut member.roles, mu.roles());
							event
						} else {
							Event::MemberUpdate(mu)
						}
					} else {
						Event::MemberUpdate(mu)
					}
				}
				GuildRoleCreate(rc) => {
					let id = rc.role.id.into();
					self.insert_role(ti!(rc.role));
					Event::RoleCreate(id)
				}
				GuildRoleDelete(rd) => {
					let id = rd.role_id.into();
					Event::RoleDelete(id, self.delete_role(id))
				}
				GuildRoleUpdate(ru) => {
					Event::RoleUpdate(ru.role.id.into(), self.insert_role(ti!(ru.role)))
				}
				ChannelCreate(cc) => {
					let id = cc.channel.id.into();
					self.insert_channel(ti!(cc.channel));
					Event::ChannelCreate(id)
				}
				ChannelDelete(cd) => {
					let id = cd.channel.id.into();
					Event::ChannelDelete(id, self.delete_channel(id))
				}
				ChannelUpdate(cu) => {
					Event::ChannelUpdate(cu.channel.id.into(), self.insert_channel(ti!(cu.channel)))
				}
				MessageReactionAdd(ra) => Event::ReactionAdd(
					ra.user_id.into(),
					ra.channel_id.into(),
					ra.message_id.into(),
					ti!(ra.emoji),
				),
				MessageReactionRemove(rr) => Event::ReactionRemove(
					rr.channel_id.into(),
					rr.message_id.into(),
					ReactionRemoveType::Single(rr.user_id.into(), ti!(rr.emoji)),
				),
				MessageReactionRemoveAll(ra) => Event::ReactionRemove(
					ra.channel_id.into(),
					ra.message_id.into(),
					ReactionRemoveType::All,
				),
				MessageReactionRemoveEmoji(re) => Event::ReactionRemove(
					re.channel_id.into(),
					re.message_id.into(),
					ReactionRemoveType::Emoji(ti!(re.emoji)),
				),
				e => Event::Other(e),
			};

			return Some(event);
		}
	}
}

pub enum GuildEvent {
	Offline,
	Online,
	Event(protocol::Event),
}

impl From<protocol::Event> for GuildEvent {
	fn from(event: protocol::Event) -> Self {
		GuildEvent::Event(event)
	}
}

impl From<GuildCreate> for GuildEvent {
	fn from(gc: GuildCreate) -> Self {
		GuildEvent::Event(gc.into())
	}
}
