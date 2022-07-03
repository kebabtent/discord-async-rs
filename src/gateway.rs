use crate::codec::{Codec, Connection, JsonCodec};
use crate::GatewayError;
use async_tungstenite::tungstenite;
use discord_types::event::EventError;
use discord_types::{command, event};
use discord_types::{Command, Intents, Payload};
use futures::stream::FusedStream;
use futures::{Sink, SinkExt, Stream, StreamExt};
use log::{debug, info};
use pin_project::pin_project;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

const API_VERSION: u8 = 10;

#[derive(Debug)]
pub enum Error {
	Shutdown,
	Ws(tungstenite::Error),
	UnexpectedEvent,
	Timeout,
	Close(Option<tungstenite::protocol::CloseFrame<'static>>),
	Decode,
	Serde(serde_json::Error),
}

impl Error {
	pub fn is_shutdown(&self) -> bool {
		match self {
			Error::Shutdown => true,
			_ => false,
		}
	}
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use Error::*;
		match self {
			Shutdown => write!(f, "Shutdown"),
			Ws(e) => fmt::Display::fmt(e, f),
			UnexpectedEvent => write!(f, "Unexpected event"),
			Timeout => write!(f, "Connection timed out"),
			Close(Some(frame)) => write!(f, "Connection closed: {}", frame),
			Close(None) => write!(f, "Connection closed"),
			Decode => write!(f, "Decode error"),
			Serde(e) => write!(f, "Serde error: {}", e),
		}
	}
}

impl std::error::Error for Error {}

impl From<tungstenite::Error> for Error {
	fn from(e: tungstenite::Error) -> Self {
		Error::Ws(e)
	}
}

impl From<EventError> for Error {
	fn from(_e: EventError) -> Self {
		Error::UnexpectedEvent
	}
}

impl From<serde_json::Error> for Error {
	fn from(e: serde_json::Error) -> Self {
		Error::Serde(e)
	}
}

macro_rules! read_event {
	($conn:expr) => {
		match $conn.next().await {
			Some(Ok(payload)) => payload.event,
			Some(Err(e)) => return Err(e.into()),
			None => return Err(Error::Ws(tungstenite::Error::ConnectionClosed)),
		}
	};
}

// TODO: compression
// TODO: ETF

enum Token<'a> {
	New(&'a str),
	Resume(&'a str, &'a str, u64),
}

pub struct Connector<'a> {
	version: u8,
	encoding: Encoding,
	compression: bool,
	properties: Option<command::ConnectionProperties>,
	token: Token<'a>,
	intents: Intents,
}

impl<'a> Connector<'a> {
	pub fn new(token: &'a str, intents: Intents) -> Self {
		Connector {
			version: API_VERSION,
			encoding: Encoding::Json,
			compression: false,
			properties: None,
			token: Token::New(token),
			intents,
		}
	}

	pub fn resume(token: &'a str, session_id: &'a str, sequence: u64, intents: Intents) -> Self {
		Connector {
			version: API_VERSION,
			encoding: Encoding::Json,
			compression: false,
			properties: None,
			token: Token::Resume(token, session_id, sequence),
			intents,
		}
	}

	pub fn is_new(&self) -> bool {
		match self.token {
			Token::New(_) => true,
			_ => false,
		}
	}

	pub fn properties(mut self, properties: command::ConnectionProperties) -> Self {
		self.properties = Some(properties);
		self
	}

	pub fn compression(mut self, _compression: bool) -> Self {
		self.compression = false;
		self
	}

	pub fn json_encoding(mut self) -> Self {
		self.encoding = Encoding::Json;
		self
	}

	pub async fn connect(self) -> Result<(Gateway, Duration, event::Event), Error> {
		let encoder = match self.encoding {
			Encoding::Json => {
				let file = if cfg!(debug_assertions) {
					Some("events.log")
				} else {
					None
				};
				Box::new(JsonCodec::new(file)) as Box<dyn Codec<Command, Payload>>
			}
		};

		let url = format!(
			"wss://gateway.discord.gg/?v={}&encoding={}",
			self.version, self.encoding
		);
		info!(
			"Connecting to gateway v{} using {} encoding",
			self.version, self.encoding
		);

		let (mut conn, _) = Connection::connect(url, encoder, Duration::from_secs(5)).await?;

		// Receive `Hello`
		let hello = read_event!(conn).expect_hello()?;

		let msg = match self.token {
			Token::New(token) => {
				// New session
				debug!("Connection established, starting new session..");
				let properties = self
					.properties
					.unwrap_or_else(|| command::ConnectionProperties {
						os: "linux".into(),
						browser: "discord-async-rs".into(),
						device: "discord-async-rs".into(),
					});
				let command = command::Identify {
					token: token.to_owned().into(),
					properties,
					compress: None,
					large_threshold: None,
					shard: None,
					presence: None,
					guild_subscriptions: None,
					intents: Some(self.intents),
				};
				command.into()
			}
			Token::Resume(token, session_id, seq) => {
				// Resume session
				debug!("Connection established, resuming previous session..");
				let command = command::Resume {
					token: token.to_owned().into(),
					session_id: session_id.to_owned().into(),
					seq,
				};
				command.into()
			}
		};
		// Send `Identify`/`Resume`
		conn.send(msg).await?;

		// We always expect to receive a message here. There are multiple possibilities.
		// For a new session we expect a `Ready`. For a resumed session we expect one of
		// - `InvalidSession`
		// - The first missed event since our disconnect
		// - `Resumed` if there are no missed events
		let event = read_event!(conn);

		let gateway = Gateway {
			conn,
			finished: false,
		};

		Ok((
			gateway,
			Duration::from_millis(hello.heartbeat_interval),
			event,
		))
	}
}

enum Encoding {
	Json,
	// Etf,
}

impl fmt::Display for Encoding {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self {
			Encoding::Json => write!(f, "json"),
			// Encoding::Etf => write!(f, "etf"),
		}
	}
}

pub enum GatewayEvent {
	Offline,
	Online,
	SessionInvalidated,
	Event(event::Event),
}

impl From<event::Event> for GatewayEvent {
	fn from(event: event::Event) -> Self {
		GatewayEvent::Event(event)
	}
}

impl From<event::GuildCreate> for GatewayEvent {
	fn from(gc: event::GuildCreate) -> Self {
		GatewayEvent::Event(event::Event::GuildCreate(gc))
	}
}

#[pin_project]
pub struct Gateway {
	#[pin]
	conn: Connection<Command, Payload>,
	finished: bool,
}

impl Gateway {
	pub async fn shutdown(&mut self) -> Result<(), GatewayError> {
		self.conn.shutdown().await
	}
}

// A fused `Connection`
// Return a `None` value forever after a single `None` or a WS error
// TODO: do we need this?
impl Stream for Gateway {
	type Item = Result<Payload, Error>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let project = self.project();
		if *project.finished {
			return Poll::Ready(None);
		}

		let res = project.conn.poll_next(cx);
		match &res {
			Poll::Ready(None) | Poll::Ready(Some(Err(Error::Ws(_)))) => *project.finished = true,
			_ => {}
		}
		res
	}
}

impl FusedStream for Gateway {
	fn is_terminated(&self) -> bool {
		self.finished
	}
}

impl Sink<Command> for Gateway {
	type Error = Error;

	fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.project().conn.poll_ready(cx)
	}

	fn start_send(self: Pin<&mut Self>, item: Command) -> Result<(), Self::Error> {
		self.project().conn.start_send(item)
	}

	fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.project().conn.poll_flush(cx)
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.project().conn.poll_close(cx)
	}
}
