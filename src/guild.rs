use crate::discord::Recv;
use crate::protocol::{GuildCreate, MessageType, ProtocolError};
use crate::Snowflake;
use crate::{protocol, types::*};
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
	event_recv: Recv<GuildEvent>,
}

impl GuildSeed {
	pub(crate) fn new(guild_create: protocol::GuildCreate, event_recv: Recv<GuildEvent>) -> Self {
		Self {
			guild_create,
			event_recv,
		}
	}

	fn inner(self) -> (protocol::GuildCreate, Recv<GuildEvent>) {
		(self.guild_create, self.event_recv)
	}
}

pub struct Guild {
	id: Snowflake,
	name: String,
	available: bool,
	state: State,
	recv: Recv<GuildEvent>,
}

impl Guild {
	pub fn new(seed: GuildSeed) -> Result<Self, ProtocolError> {
		let (gc, recv) = seed.inner();

		let state = State::Stateful {
			channels: HashMap::new(),
			roles: HashMap::new(),
			members: HashMap::new(),
		};

		let mut guild = Self {
			id: gc.id,
			name: String::new(),
			available: false,
			state,
			recv,
		};
		guild.update(gc)?;
		Ok(guild)
	}

	fn update(&mut self, gc: GuildCreate) -> Result<(), ProtocolError> {
		self.name = gc.name;

		let channels;
		let roles;
		let members;

		match &mut self.state {
			State::Stateful {
				channels: c,
				roles: r,
				members: m,
			} => {
				channels = c;
				roles = r;
				members = m;
			}
			State::Stateless { .. } => return Ok(()),
		}

		for channel in gc.channels {
			channels.insert(channel.id, channel.try_into()?);
		}

		for role in gc.roles {
			roles.insert(role.id, role.into());
		}

		for member in gc.members {
			members.insert(member.user.id, member.try_into()?);
		}

		info!(
			"Loaded guild '{}' ({} channels, {} roles, {}/{} members)",
			self.name,
			channels.len(),
			roles.len(),
			members.len(),
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

	pub fn channel(&self, id: Snowflake) -> Option<&Channel> {
		self.state.channel(id)
	}

	pub fn role(&self, id: Snowflake) -> Option<&Role> {
		self.state.role(id)
	}

	pub fn member(&self, id: Snowflake) -> Option<&Member> {
		self.state.member(id)
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
				MessageCreate(protocol::MessageCreate { mut message }) => {
					// Skip webhook messages
					if message.webhook_id.is_some() {
						continue;
					}

					// Skip messages without a member
					let member_id = match message.member.take() {
						Some(m) => {
							let id = message.author.id;
							self.state.update_member(ti!((m, message.author.clone())));
							id
						}
						None => continue,
					};

					if message.message_type != MessageType::Default {
						continue;
					}

					Event::MessageCreate(ti!((message, member_id)))
				}
				MessageDelete(md) => Event::MessageDelete {
					id: md.id,
					channel_id: md.channel_id,
				},
				MessageDeleteBulk(md) => Event::MessageDeleteBulk {
					ids: md.ids,
					channel_id: md.channel_id,
				},
				MessageUpdate(protocol::MessageUpdate { mut message }) => {
					// Skip webhook messages
					if message.webhook_id.is_some() {
						continue;
					}

					// Skip messages without a member
					let member_id = match message.member.take() {
						Some(m) => {
							let id = message.author.id;
							self.state.update_member(ti!((m, message.author.clone())));
							id
						}
						None => continue,
					};

					if message.message_type != MessageType::Default {
						continue;
					}

					Event::MessageUpdate(ti!((message, member_id)))
				}
				GuildMemberAdd(ma) => {
					let id = ma.member.user.id;
					self.state.update_member(ti!(ma.member));
					Event::MemberJoin(id)
				}
				GuildMemberRemove(mr) => {
					let member = match self.state.delete_member(mr.user.id) {
						Some(m) => Either::Left(m),
						None => Either::<_, User>::Right(ti!(mr.user)),
					};
					Event::MemberLeave(member)
				}
				GuildMemberUpdate(mu) => {
					let id = mu.user.id;

					match &mut self.state {
						State::Stateful { members, .. } => {
							// Figure out which field changed and fire appropriate event
							let premium_since =
								t!(mu.premium_since.as_ref().map(|s| s.parse()).transpose());
							if let Some(member) = members.get_mut(&id) {
								if mu.nick != member.nickname {
									Event::MemberChangeNickname {
										id,
										old_nickname: mem::replace(&mut member.nickname, mu.nick),
									}
								} else if mu.user.username != member.user.username
									|| mu.user.discriminator != member.user.discriminator
								{
									Event::MemberChangeUsername {
										id,
										old_username: mem::replace(
											&mut member.user.username,
											mu.user.username,
										),
										old_discriminator: mem::replace(
											&mut member.user.discriminator,
											mu.user.discriminator,
										),
									}
								} else if mu.user.avatar != member.user.avatar {
									Event::MemberChangeAvatar {
										id,
										old_avatar: mem::replace(
											&mut member.user.avatar,
											mu.user.avatar,
										),
									}
								} else if premium_since != member.premium_since {
									Event::MemberChangePremium {
										id,
										old_premium_since: mem::replace(
											&mut member.premium_since,
											premium_since,
										),
									}
								} else if let Some(role_id) = mu
									.roles
									.symmetric_difference(&member.roles)
									.next()
									.map(|r| *r)
								{
									let event = if mu.roles.contains(&role_id) {
										Event::MemberAddRole {
											user_id: id,
											role_id,
										}
									} else {
										Event::MemberRemoveRole {
											user_id: id,
											role_id,
										}
									};
									mem::replace(&mut member.roles, mu.roles);
									event
								} else {
									Event::MemberUpdate(mu)
								}
							} else {
								Event::MemberUpdate(mu)
							}
						}
						_ => Event::MemberUpdate(mu),
					}
				}
				GuildRoleCreate(rc) => {
					let id = rc.role.id;
					self.state.update_role(ti!(rc.role));
					Event::RoleCreate(id)
				}
				GuildRoleDelete(rd) => {
					Event::RoleDelete(rd.role_id, self.state.delete_role(rd.role_id))
				}
				GuildRoleUpdate(ru) => {
					Event::RoleUpdate(ru.role.id, self.state.update_role(ti!(ru.role)))
				}
				ChannelCreate(cc) => {
					let id = cc.channel.id;
					self.state.update_channel(ti!(cc.channel));
					Event::ChannelCreate(id)
				}
				ChannelDelete(cd) => {
					Event::ChannelDelete(cd.channel.id, self.state.delete_channel(cd.channel.id))
				}
				ChannelUpdate(cu) => {
					Event::ChannelUpdate(cu.channel.id, self.state.update_channel(ti!(cu.channel)))
				}
				MessageReactionAdd(ra) => Event::ReactionAdd {
					user_id: ra.user_id,
					channel_id: ra.channel_id,
					message_id: ra.message_id,
					emoji: ti!(ra.emoji),
				},
				MessageReactionRemove(rr) => Event::ReactionRemove {
					channel_id: rr.channel_id,
					message_id: rr.message_id,
					remove_type: ReactionRemoveType::Single {
						user_id: rr.user_id,
						emoji: ti!(rr.emoji),
					},
				},
				MessageReactionRemoveAll(ra) => Event::ReactionRemove {
					channel_id: ra.channel_id,
					message_id: ra.message_id,
					remove_type: ReactionRemoveType::All,
				},
				MessageReactionRemoveEmoji(re) => Event::ReactionRemove {
					channel_id: re.channel_id,
					message_id: re.message_id,
					remove_type: ReactionRemoveType::Emoji(ti!(re.emoji)),
				},
				e => Event::Other(e),
			};

			return Some(event);
		}
	}
}

enum State {
	Stateful {
		channels: HashMap<Snowflake, Channel>,
		roles: HashMap<Snowflake, Role>,
		members: HashMap<Snowflake, Member>,
	},
	Stateless {
		channel: Option<Channel>,
		role: Option<Role>,
		member: Option<Member>,
	},
}

impl State {
	fn is_stateful(&self) -> bool {
		match self {
			State::Stateful { .. } => true,
			State::Stateless { .. } => false,
		}
	}

	fn channel(&self, id: Snowflake) -> Option<&Channel> {
		match self {
			State::Stateful { channels, .. } => channels.get(&id),
			State::Stateless { .. } => None,
		}
	}

	fn update_channel(&mut self, c: Channel) -> Option<Channel> {
		match self {
			State::Stateful { channels, .. } => channels.insert(c.id, c),
			State::Stateless { channel, .. } => {
				*channel = Some(c);
				None
			}
		}
	}

	fn delete_channel(&mut self, id: Snowflake) -> Option<Channel> {
		match self {
			State::Stateful { channels, .. } => channels.remove(&id),
			_ => None,
		}
	}

	fn role(&self, id: Snowflake) -> Option<&Role> {
		match self {
			State::Stateful { roles, .. } => roles.get(&id),
			State::Stateless { role, .. } => match role.as_ref() {
				Some(role) if role.id == id => Some(role),
				_ => None,
			},
		}
	}

	fn update_role(&mut self, r: Role) -> Option<Role> {
		match self {
			State::Stateful { roles, .. } => roles.insert(r.id, r),
			State::Stateless { role, .. } => {
				*role = Some(r);
				None
			}
		}
	}

	fn delete_role(&mut self, id: Snowflake) -> Option<Role> {
		match self {
			State::Stateful { roles, .. } => roles.remove(&id),
			_ => None,
		}
	}

	fn member(&self, id: Snowflake) -> Option<&Member> {
		match self {
			State::Stateful { members, .. } => members.get(&id).map(|m| m.into()),
			State::Stateless { member, .. } => match member.as_ref() {
				Some(member) if member.user.id == id => Some(member),
				_ => None,
			},
		}
	}

	fn update_member(&mut self, m: Member) -> Option<Member> {
		match self {
			State::Stateful { members, .. } => members.insert(m.user.id, m),
			State::Stateless { member, .. } => {
				*member = Some(m);
				None
			}
		}
	}

	fn delete_member(&mut self, id: Snowflake) -> Option<Member> {
		match self {
			State::Stateful { members, .. } => members.remove(&id),
			_ => None,
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
