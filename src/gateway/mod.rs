use crate::protocol;
use async_tungstenite::tokio::{connect_async, TokioAdapter};
use async_tungstenite::WebSocketStream;
use futures::{Sink, Stream};
use futures::{SinkExt, StreamExt};
use log::{debug, info, trace};
use pin_project::pin_project;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::TcpStream;

mod command;
mod encoder;

pub use command::{Command, Intents};
use encoder::{Encoder, JsonEncoder};

pub(crate) type AutoStream<S> =
	async_tungstenite::stream::Stream<S, async_tls::client::TlsStream<S>>;

macro_rules! read_command {
	($conn:expr, $fnc:ident) => {
		match $conn.next().await {
			Some(Ok(payload)) => payload.event.$fnc()?,
			Some(Err(e)) => return Err(e.into()),
			None => return Err(Error::Ws(tungstenite::Error::ConnectionClosed)),
			}
	};
}

// TODO: compression
// TODO: ETF

#[derive(Debug)]
pub enum Error {
	Ws(tungstenite::Error),
	UnexpectedEvent,
	Close(Option<tungstenite::protocol::CloseFrame<'static>>),
	Decode,
	DecodeJson(serde_json::Error),
}

impl From<tungstenite::Error> for Error {
	fn from(error: tungstenite::Error) -> Self {
		Error::Ws(error)
	}
}

impl From<protocol::EventError> for Error {
	fn from(_error: protocol::EventError) -> Self {
		Error::UnexpectedEvent
	}
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Properties {
	#[serde(rename = "$os")]
	os: String,
	#[serde(rename = "$browser")]
	browser: String,
	#[serde(rename = "$device")]
	device: String,
}

impl Default for Properties {
	fn default() -> Self {
		Self {
			os: String::from("linux"),
			browser: String::from("discord-async-rs"),
			device: String::from("discord-async-rs"),
		}
	}
}

enum Token<'a> {
	New(&'a str),
	Resume(&'a str, &'a str, u64),
}

pub struct Connector<'a> {
	version: u8,
	encoding: Encoding,
	compression: bool,
	properties: Option<Properties>,
	token: Token<'a>,
}

impl<'a> Connector<'a> {
	pub fn new(token: &'a str) -> Self {
		Connector {
			version: 6,
			encoding: Encoding::Json,
			compression: false,
			properties: None,
			token: Token::New(token),
		}
	}

	pub fn resume(token: &'a str, session_id: &'a str, sequence: u64) -> Self {
		Connector {
			version: 6,
			encoding: Encoding::Json,
			compression: false,
			properties: None,
			token: Token::Resume(token, session_id, sequence),
		}
	}

	pub fn properties(mut self, properties: Properties) -> Self {
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

	pub async fn connect(self) -> Result<(Gateway, Duration, Option<protocol::Ready>), Error> {
		let encoder = match self.encoding {
			Encoding::Json => Box::new(JsonEncoder::new()),
		};

		let url = format!(
			"wss://gateway.discord.gg/?v={}&encoding={}",
			self.version, self.encoding
		);
		info!(
			"Connecting to gateway v{} using {} encoding",
			self.version, self.encoding
		);

		let (conn, _) = connect_async(url).await?;
		let mut conn = Connection { conn, encoder };

		// Receive `Hello`
		let hello = read_command!(conn, expect_hello);

		let msg = match self.token {
			Token::New(token) => {
				// New session
				debug!("Connection established, starting new session..");
				let properties = self.properties.unwrap_or_else(|| Properties::default());
				command::Identify::new(token.into(), properties).into()
			}
			Token::Resume(token, session_id, sequence) => {
				// Resume session
				debug!("Connection established, resuming previous session..");
				command::Resume::new(token.into(), session_id.into(), sequence).into()
			}
		};
		// Send `Identify`/`Resume`
		conn.send(msg).await?;

		let ready = if let Token::New(_) = self.token {
			// Receive `Ready`
			let ready = read_command!(conn, expect_ready);
			debug!("Handshake complete");
			Some(ready)
		} else {
			None
		};

		let gateway = Gateway { conn };

		Ok((
			gateway,
			Duration::from_millis(hello.heartbeat_interval),
			ready,
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

#[pin_project]
pub struct Gateway {
	#[pin]
	conn: Connection,
}

impl Stream for Gateway {
	type Item = Result<protocol::Payload, Error>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		self.project().conn.poll_next(cx)
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

#[pin_project]
pub struct Connection {
	#[pin]
	conn: WebSocketStream<AutoStream<TokioAdapter<TcpStream>>>,
	encoder: Box<dyn Encoder>,
}

impl Stream for Connection {
	type Item = Result<protocol::Payload, Error>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let project = self.project();
		match project.conn.poll_next(cx) {
			Poll::Ready(Some(Ok(message))) => match project.encoder.decode(message) {
				Ok(p) => {
					if p.event.is_heartbeat_ack() {
						trace!("RECV {}", p);
					} else {
						debug!("RECV {}", p);
					}
					Poll::Ready(Some(Ok(p)))
				}
				Err(e) => Poll::Ready(Some(Err(e))),
			},
			Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e.into()))),
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}
}

impl Sink<Command> for Connection {
	type Error = Error;

	fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.project().conn.poll_ready(cx).map_err(|e| e.into())
	}

	fn start_send(self: Pin<&mut Self>, item: Command) -> Result<(), Self::Error> {
		if item.is_heartbeat() {
			trace!("SEND {}", item);
		} else {
			debug!("SEND {}", item);
		}
		let project = self.project();
		let message = project.encoder.encode(item);
		project.conn.start_send(message).map_err(|e| e.into())
	}

	fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.project().conn.poll_flush(cx).map_err(|e| e.into())
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.project().conn.poll_close(cx).map_err(|e| e.into())
	}
}
