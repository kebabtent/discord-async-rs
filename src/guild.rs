#[cfg(feature = "voice")]
use crate::voice;
use crate::{Client, Error, GatewayEvent};
use discord_types::event;
use discord_types::{
	ApplicationCommand, ApplicationId, Channel, ChannelId, Event, GuildId, Member, Role, RoleId,
	UserId,
};
use futures::{Stream, StreamExt};
use log::{debug, info};
use std::collections::HashMap;

pub struct Guild<S> {
	id: GuildId,
	user_id: UserId,
	application_id: ApplicationId,
	name: String,
	available: bool,
	channels: HashMap<ChannelId, Channel>,
	roles: HashMap<RoleId, Role>,
	member_count: usize,
	members: HashMap<UserId, Member>,
	commands: HashMap<String, ApplicationCommand>,
	stream: S,
	client: Client,
}

impl<S> Guild<S> {
	pub fn client(&self) -> Client {
		self.client.clone()
	}

	fn update(&mut self, guild: discord_types::Guild) {
		self.name = guild.name;
		self.channels = guild.channels.into_iter().map(|c| (c.id, c)).collect();
		self.roles = guild.roles.into_iter().map(|r| (r.id, r)).collect();
		for (id, member) in guild
			.members
			.into_iter()
			.filter_map(|m| Some((m.user.as_ref()?.id, m)))
		{
			self.members.insert(id, member);
		}

		let member_count = self.members.len();
		let actual_member_count = guild.member_count.unwrap_or(0) as usize;
		if member_count < actual_member_count {
			debug!("Requesting all guild members");
			let _ = self.client.request_guild_members(self.id);
		}

		info!(
			"Loaded guild '{}' ({} channels, {} roles, {}/{} members, {} commands)",
			self.name,
			self.channels.len(),
			self.roles.len(),
			member_count,
			actual_member_count,
			self.commands.len(),
		);
	}

	async fn load_commands(&mut self) -> Result<(), Error> {
		self.commands = self
			.client
			.commands(self.application_id, self.id)
			.await?
			.into_iter()
			.map(|c| (c.name.clone(), c))
			.collect();
		Ok(())
	}

	pub fn id(&self) -> GuildId {
		self.id
	}

	pub fn user_id(&self) -> UserId {
		self.user_id
	}

	pub fn application_id(&self) -> ApplicationId {
		self.application_id
	}

	pub fn name(&self) -> &str {
		&self.name
	}

	pub fn available(&self) -> bool {
		self.available
	}

	#[cfg(feature = "voice")]
	pub fn create_player(&self) -> (voice::Updater, voice::Controller, voice::Listener) {
		let (player, updater, controller, listener) =
			voice::Player::new(self.id, self.user_id, self.client.clone());
		player.spawn();
		(updater, controller, listener)
	}

	pub fn channels(&self) -> impl Iterator<Item = &Channel> {
		self.channels.values()
	}

	pub fn channel<T: Into<ChannelId>>(&self, id: T) -> Option<&Channel> {
		self.channels.get(&id.into())
	}

	pub fn roles(&self) -> impl Iterator<Item = &Role> {
		self.roles.values()
	}

	pub fn role<T: Into<RoleId>>(&self, id: T) -> Option<&Role> {
		self.roles.get(&id.into())
	}

	pub fn member_count(&self) -> usize {
		self.member_count
	}

	pub fn members(&self) -> impl Iterator<Item = &Member> {
		self.members.values()
	}

	pub fn member<T: Into<UserId>>(&self, id: T) -> Option<&Member> {
		self.members.get(&id.into())
	}

	pub fn member_role_position(&self, member: &Member) -> u16 {
		member
			.roles
			.iter()
			.filter_map(|&id| self.role(id))
			.map(|r| r.position)
			.max()
			.unwrap_or(0)
	}

	pub fn command(&self, name: &str) -> Option<&ApplicationCommand> {
		self.commands.get(name)
	}
}

impl<S> Guild<S>
where
	S: Stream<Item = GatewayEvent> + Unpin + Sync + Send + 'static,
{
	pub(crate) async fn new(
		stream: S,
		gc: event::GuildCreate,
		client: Client,
		user_id: UserId,
		application_id: ApplicationId,
	) -> Result<Self, Error> {
		let mut guild = Self {
			id: gc.guild.id,
			user_id,
			application_id,
			name: gc.guild.name.clone(),
			available: false,
			channels: HashMap::new(),
			roles: HashMap::new(),
			member_count: 0,
			members: HashMap::new(),
			commands: HashMap::new(),
			stream,
			client,
		};
		guild.load_commands().await?;
		guild.update(gc.guild);
		Ok(guild)
	}

	pub async fn next(&mut self) -> Option<GatewayEvent> {
		// Some `GatewayEvent`s might not be forwarded, so we loop until we get one
		loop {
			let event = match self.stream.next().await? {
				GatewayEvent::Event(e) => e,
				x => return Some(x),
			};

			use Event::*;
			let event = match event {
				GuildCreate(gc) => {
					self.update(gc.guild.clone());
					GuildCreate(gc)
				}
				GuildUpdate(gu) => {
					self.update(gu.guild.clone());
					GuildUpdate(gu)
				}
				e @ MessageCreate(_) | e @ MessageUpdate(_) | e @ MessageDelete(_) => e,
				GuildMemberAdd(ma) => {
					if let Some(user_id) = ma.member.user.as_ref().map(|u| u.id) {
						let member = ma.member.clone();
						self.members.insert(user_id, member);
					}
					GuildMemberAdd(ma)
				}
				GuildMemberUpdate(mu) => {
					if let Some(member) = self.members.get_mut(&mu.user.id) {
						let event::GuildMemberUpdate {
							roles,
							user,
							nick,
							premium_since,
							..
						} = mu.clone();
						member.roles = roles;
						member.user = Some(user);
						member.nick = nick;
						member.premium_since = premium_since;
					}
					GuildMemberUpdate(mu)
				}
				GuildMemberRemove(mr) => {
					self.members.remove(&mr.user.id);
					GuildMemberRemove(mr)
				}
				GuildMembersChunk(mc) => {
					for (id, member) in mc
						.members
						.iter()
						.filter_map(|m| Some((m.user.as_ref()?.id, m)))
					{
						self.members.insert(id, member.clone());
					}
					GuildMembersChunk(mc)
				}
				GuildRoleCreate(rc) => {
					let role = rc.role.clone();
					self.roles.insert(role.id, role);
					GuildRoleCreate(rc)
				}
				GuildRoleUpdate(ru) => {
					let role = ru.role.clone();
					self.roles.insert(role.id, role);
					GuildRoleUpdate(ru)
				}
				GuildRoleDelete(rd) => {
					self.roles.remove(&rd.role_id);
					for (_, member) in self.members.iter_mut() {
						member.roles.remove(&rd.role_id);
					}
					GuildRoleDelete(rd)
				}
				ChannelCreate(cc) => {
					let channel = cc.channel.clone();
					self.channels.insert(channel.id, channel);
					ChannelCreate(cc)
				}
				ChannelUpdate(cu) => {
					let channel = cu.channel.clone();
					self.channels.insert(channel.id, channel);
					ChannelUpdate(cu)
				}
				ChannelDelete(cd) => {
					self.channels.remove(&cd.channel.id);
					ChannelDelete(cd)
				}
				/*MessageReactionAdd(ra) => Event::ReactionAdd(
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
				),*/
				ApplicationCommandCreate(cc) => {
					let command = cc.command.clone();
					self.commands.insert(command.name.clone(), command);
					ApplicationCommandCreate(cc)
				}
				ApplicationCommandUpdate(cu) => {
					let command = cu.command.clone();
					self.commands.insert(command.name.clone(), command);
					ApplicationCommandUpdate(cu)
				}
				ApplicationCommandDelete(cd) => {
					self.commands.remove(&cd.command.name);
					ApplicationCommandDelete(cd)
				}
				e @ InteractionCreate(_) => e,
				GuildDelete(gd) => {
					// TODO
					GuildDelete(gd)
				}
				e @ VoiceStateUpdate(_) => e,
				e @ VoiceServerUpdate(_) => e,
				e @ Hello(_)
				| e @ Ready(_)
				| e @ Resumed
				| e @ InvalidSession(_)
				| e @ HeartbeatAck
				| e @ Unknown(_) => e,
			};

			return Some(GatewayEvent::Event(event));
		}
	}
}
