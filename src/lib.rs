pub use crate::client::{
	ButtonComponent, Client, Error as ClientError, OptionalResult, RowComponent,
};
pub use crate::discord::{Builder, Discord};
pub use crate::gateway::{Connector, Error as GatewayError, Gateway};
pub use crate::guild::{Guild, GuildEvent, GuildSeed};
pub use discord_types as types;
use futures::channel::mpsc;
use serde::Deserialize;
use std::fmt;

mod client;
mod codec;
mod discord;
mod gateway;
mod guild;
pub mod interaction;
// pub mod message;
#[cfg(feature = "voice")]
pub mod voice;

pub(crate) type Send<T> = mpsc::Sender<T>;
pub(crate) type Recv<T> = mpsc::Receiver<T>;

#[derive(Debug)]
pub enum Error {
	Gateway(GatewayError),
	Protocol(ProtocolError),
	Client(client::Error),
	Api(ApiError),
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Error::Gateway(e) => write!(f, "Gateway error: {}", e),
			Error::Protocol(e) => write!(f, "Protocol error: {}", e),
			Error::Client(e) => write!(f, "Client error: {}", e),
			Error::Api(e) => write!(f, "API error: {}", e),
		}
	}
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		None
	}
}

impl From<gateway::Error> for Error {
	fn from(e: gateway::Error) -> Self {
		Error::Gateway(e)
	}
}

impl From<ProtocolError> for Error {
	fn from(e: ProtocolError) -> Self {
		Error::Protocol(e)
	}
}

impl From<client::Error> for Error {
	fn from(e: client::Error) -> Self {
		Error::Client(e)
	}
}

impl From<ApiError> for Error {
	fn from(e: ApiError) -> Self {
		Error::Api(e)
	}
}

#[derive(Debug)]
pub enum ProtocolError {
	InvalidField(&'static str),
	MissingField(&'static str),
}

impl fmt::Display for ProtocolError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			ProtocolError::InvalidField(e) => write!(f, "Invalid field '{}'", e),
			ProtocolError::MissingField(e) => write!(f, "Missing field '{}'", e),
		}
	}
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApiError {
	code: u32,
	message: String,
}

impl ApiError {
	pub fn code(&self) -> u32 {
		self.code
	}
	pub fn message(&self) -> &str {
		&self.message
	}
}

impl fmt::Display for ApiError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "API error {}: {}", self.code, self.message)
	}
}
