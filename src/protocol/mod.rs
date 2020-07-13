use serde::Deserialize;
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::fmt;

pub mod event;
pub mod request;

pub use event::*;
pub use request::*;

#[derive(Debug)]
pub enum ProtocolError {
	InvalidField(&'static str),
	MissingField(&'static str),
}

impl std::error::Error for ProtocolError {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		None
	}
}

impl fmt::Display for ProtocolError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			ProtocolError::InvalidField(e) => write!(f, "Invalid field '{}'", e),
			ProtocolError::MissingField(e) => write!(f, "Missing field '{}'", e),
		}
	}
}

#[derive(Clone, Copy, Debug, Deserialize_repr, Serialize_repr)]
#[repr(u8)]
pub enum ChannelType {
	DirectMessage = 1,
	GroupDirectMessage = 3,
	GuildText = 0,
	GuildVoice = 2,
	GuildCategory = 4,
	GuildNews = 5,
	GuildStore = 6,
}

#[derive(Clone, Copy, Debug, Deserialize_repr, Eq, PartialEq, Serialize_repr)]
#[repr(u8)]
pub enum MessageType {
	Default = 0,
	RecipientAdd = 1,
	RecipientRemove = 2,
	Call = 3,
	ChannelNameChange = 4,
	ChannelIconChange = 5,
	ChannelPinnedMessage = 6,
	GuildMemberJoin = 7,
	UserPremiumGuildSubscription = 8,
	UserPremiumGuildSubscriptionTier1 = 9,
	UserPremiumGuildSubscriptionTier2 = 10,
	UserPremiumGuildSubscriptionTier3 = 11,
	ChannelFollowAdd = 12,
	GuildDiscoveryDisqualified = 14,
	GuildDiscoveryRequalified = 15,
}

bitflags::bitflags! {
	#[derive(Deserialize)]
	#[serde(transparent)]
	pub struct UserFlags: u32 {
		const DISCORD_EMPLOYEE = 1 << 0;
		const DISCORD_PARTNER = 1 << 1;
		const HYPESQUAD_EVENTS = 1 << 2;
		const BUG_HUNTER_LEVEL_1 = 1 << 3;
		const HOUSE_BRAVERY = 1 << 6;
		const HOUSE_BRILLIANCE = 1 << 7;
		const HOUSE_BALANCE = 1 << 8;
		const EARLY_SUPPORTER = 1 << 9;
		const TEAM_USER = 1 << 10;
		const SYSTEM = 1 << 12;
		const BUG_HUNTER_LEVEL_2 = 1 << 14;
		const VERIFIED_BOT = 1 << 16;
		const VERIFIED_BOT_DEVELOPER = 1 << 17;
	}
}
