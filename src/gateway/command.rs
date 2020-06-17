use super::Properties;
use serde::Serialize;
use std::fmt;

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Command {
	Identify(Identify),
	Resume(Resume),
	Heartbeat(Heartbeat),
	RequestGuildMembers(RequestGuildMembers),
	UpdateVoiceState(UpdateVoiceState),
	UpdateStatus(UpdateStatus),
}

impl Command {
	pub fn op(&self) -> u8 {
		match self {
			Command::Identify(_) => 2,
			Command::Resume(_) => 6,
			Command::Heartbeat(_) => 1,
			Command::RequestGuildMembers(_) => 8,
			Command::UpdateVoiceState(_) => 4,
			Command::UpdateStatus(_) => 3,
		}
	}

	pub fn is_heartbeat(&self) -> bool {
		match self {
			Command::Heartbeat(_) => true,
			_ => false,
		}
	}

	pub fn heartbeat<T: Into<Heartbeat>>(heartbeat: T) -> Self {
		Command::Heartbeat(heartbeat.into())
	}
}

impl fmt::Display for Command {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Command::Identify(_) => write!(f, "Identify"),
			Command::Resume(_) => write!(f, "Resume"),
			Command::Heartbeat(c) => write!(f, "Heartbeat({})", c),
			Command::RequestGuildMembers(_) => write!(f, "RequestGuildMembers"),
			Command::UpdateVoiceState(_) => write!(f, "UpdateVoiceState"),
			Command::UpdateStatus(_) => write!(f, "UpdateStatus"),
		}
	}
}

#[derive(Debug, Serialize)]
pub struct Activity {}

#[derive(Debug, Serialize)]
pub struct Presence {
	since: Option<u64>,
	game: Option<Activity>,
	status: String,
	afk: bool,
}

bitflags::bitflags! {
	#[derive(Serialize)]
	#[serde(transparent)]
	pub struct Intents: u16 {
		const GUILDS = 1 << 0;
		const GUILD_MEMBERS = 1 << 1;
		const GUILD_BANS = 1 << 2;
		const GUILD_EMOJIS = 1 << 3;
		const GUILD_INTEGRATIONS = 1 << 4;
		const GUILD_WEBHOOKS = 1 << 5;
		const GUILD_INVITES = 1 << 6;
		const GUILD_VOICE_STATES = 1 << 7;
		const GUILD_PRESENCES = 1 << 8;
		const GUILD_MESSAGES = 1 << 9;
		const GUILD_MESSAGE_REACTIONS = 1 << 10;
		const GUILD_MESSAGE_TYPING = 1 << 11;
		const DIRECT_MESSAGES = 1 << 12;
		const DIRECT_MESSAGE_REACTIONS = 1 << 13;
		const DIRECT_MESSAGE_TYPING = 1 << 14;

		const GUILD_ALL = Self::GUILDS.bits | Self::GUILD_MEMBERS.bits | Self::GUILD_BANS.bits |
			Self::GUILD_EMOJIS.bits | Self::GUILD_INTEGRATIONS.bits | Self::GUILD_WEBHOOKS.bits |
			Self::GUILD_INVITES.bits | Self::GUILD_VOICE_STATES.bits | Self::GUILD_PRESENCES.bits |
			Self::GUILD_MESSAGES.bits | Self::GUILD_MESSAGE_REACTIONS.bits |
			Self::GUILD_MESSAGE_TYPING.bits;
		const DIRECT_MESSAGE_ALL = Self::DIRECT_MESSAGES.bits | Self::DIRECT_MESSAGE_REACTIONS.bits |
			Self::DIRECT_MESSAGE_TYPING.bits;
		const ALL = Self::GUILD_ALL.bits | Self::DIRECT_MESSAGE_ALL.bits;
	}
}

#[derive(Debug, Serialize)]
pub struct Identify {
	token: String,
	properties: Properties,
	#[serde(skip_serializing_if = "Option::is_none")]
	compress: Option<bool>,
	#[serde(skip_serializing_if = "Option::is_none")]
	large_threshold: Option<u8>,
	#[serde(skip_serializing_if = "Option::is_none")]
	shard: Option<(u8, u8)>,
	#[serde(skip_serializing_if = "Option::is_none")]
	presence: Option<Presence>,
	#[serde(skip_serializing_if = "Option::is_none")]
	guild_subscriptions: Option<bool>,
	#[serde(skip_serializing_if = "Option::is_none")]
	intents: Option<Intents>,
}

impl Identify {
	pub fn new(token: String, properties: Properties) -> Self {
		Self {
			token,
			properties,
			compress: None,
			large_threshold: None,
			shard: None,
			presence: None,
			guild_subscriptions: None,
			intents: Some(
				Intents::GUILD_ALL
					^ Intents::GUILD_WEBHOOKS
					// ^ Intents::GUILD_PRESENCES
					^ Intents::GUILD_MESSAGE_TYPING,
			),
		}
	}
}

impl From<Identify> for Command {
	fn from(identify: Identify) -> Command {
		Command::Identify(identify)
	}
}

#[derive(Debug, Serialize)]
pub struct Resume {
	token: String,
	session_id: String,
	seq: u64,
}

impl Resume {
	pub fn new(token: String, session_id: String, seq: u64) -> Self {
		Self {
			token,
			session_id,
			seq,
		}
	}
}

impl From<Resume> for Command {
	fn from(resume: Resume) -> Command {
		Command::Resume(resume)
	}
}

#[derive(Debug, Serialize)]
pub struct Heartbeat(u64);

impl From<u64> for Heartbeat {
	fn from(sequence: u64) -> Self {
		Heartbeat(sequence)
	}
}

impl fmt::Display for Heartbeat {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.0)
	}
}

#[derive(Debug, Serialize)]
pub struct RequestGuildMembers {}

#[derive(Debug, Serialize)]
pub struct UpdateVoiceState {}

#[derive(Debug, Serialize)]
pub struct UpdateStatus {}
