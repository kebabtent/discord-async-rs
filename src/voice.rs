pub use self::opus::OpusStream;
pub use self::player::*;
use crate::gateway::Error as GatewayError;
use discord_types::voice::EventError;
use std::fmt;

mod connection;
mod gateway;
pub mod opus;
pub mod pcm;
mod player;
mod socket;
pub mod source;

pub const SAMPLE_RATE: usize = 48_000;
pub const FRAME_LENGTH_MS: usize = 20;

#[derive(Debug)]
pub enum Error {
	MissingEndpoint,
	UnexpectedEvent,
	Gateway(GatewayError),
	Encode(EncodeError),
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use Error::*;
		match self {
			MissingEndpoint => write!(f, "Missing endpoint"),
			UnexpectedEvent => write!(f, "Unexpected event"),
			Gateway(e) => fmt::Display::fmt(e, f),
			Encode(e) => fmt::Display::fmt(e, f),
		}
	}
}

impl std::error::Error for Error {}

impl From<EventError> for Error {
	fn from(_e: EventError) -> Self {
		Error::UnexpectedEvent
	}
}

impl From<GatewayError> for Error {
	fn from(e: GatewayError) -> Self {
		Error::Gateway(e)
	}
}

impl From<EncodeError> for Error {
	fn from(e: EncodeError) -> Self {
		Error::Encode(e)
	}
}

#[derive(Debug)]
pub enum EncodeError {
	Io(std::io::Error),
	Opus(::opus::Error),
	FrameSize,
}

impl From<std::io::Error> for EncodeError {
	fn from(e: std::io::Error) -> Self {
		EncodeError::Io(e)
	}
}

impl From<::opus::Error> for EncodeError {
	fn from(e: ::opus::Error) -> Self {
		EncodeError::Opus(e)
	}
}

impl fmt::Display for EncodeError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			EncodeError::Io(e) => fmt::Display::fmt(e, f),
			EncodeError::Opus(e) => fmt::Display::fmt(e, f),
			EncodeError::FrameSize => write!(f, "Invalid frame size"),
		}
	}
}
