use crate::codec::{Connection, JsonCodec};
use crate::gateway::Error as GatewayError;
use async_tungstenite::tungstenite;
use discord_types::voice::command;
use discord_types::voice::{Command, Event, Ready, Speaking};
use discord_types::{GuildId, SpeakingFlags, UserId};
use futures::channel::mpsc;
use futures::future::join;
use futures::stream::{FusedStream, SplitSink, SplitStream};
use futures::{select, FutureExt};
use futures::{Sink, SinkExt, Stream, StreamExt};
use log::{debug, info, warn};
use pin_project::pin_project;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time;
use tokio_stream::wrappers::IntervalStream;

type CowString = std::borrow::Cow<'static, str>;

trait Error: std::error::Error + Send + Sync + 'static {}

impl<T> Error for T where T: std::error::Error + Send + Sync + 'static {}

impl<E> From<E> for Box<dyn Error>
where
	E: Error,
{
	fn from(e: E) -> Self {
		Box::new(e) as Box<dyn Error>
	}
}

pub type Listener = mpsc::Receiver<Event>;
const API_VERSION: u8 = 4;

macro_rules! read_command {
	($conn:expr, $fnc:ident) => {
		match $conn.next().await {
			Some(Ok(event)) => event.$fnc()?,
			Some(Err(e)) => return Err(e.into()),
			None => return Err(GatewayError::Ws(tungstenite::Error::ConnectionClosed).into()),
		}
	};
}

#[derive(Clone, Debug)]
pub struct Credentials {
	pub guild_id: GuildId,
	pub user_id: UserId,
	pub session_id: String,
	pub token: String,
	pub endpoint: String,
}

#[derive(Debug)]
pub struct Controller {
	inner: mpsc::Sender<Command>,
}

impl Controller {
	async fn send(&mut self, item: Command) -> bool {
		if let Err(e) = self.inner.send(item).await {
			warn!("Controller: {}", e);
			false
		} else {
			true
		}
	}

	pub async fn speaking(&mut self, ssrc: u32, priority: bool) -> bool {
		let mut speaking = SpeakingFlags::MICROPHONE;
		if priority {
			speaking |= SpeakingFlags::PRIORITY;
		}
		self.send(Command::Speaking(Speaking {
			speaking,
			delay: 0,
			ssrc,
		}))
		.await
	}

	pub async fn select_protocol<T: Into<CowString>>(&mut self, addr: SocketAddr, mode: T) -> bool {
		self.send(Command::SelectProtocol(command::SelectProtocol {
			protocol: "udp".into(),
			data: command::SelectProtocolData {
				address: addr.ip().to_string().into(),
				port: addr.port(),
				mode: mode.into(),
			},
		}))
		.await
	}
}

impl From<mpsc::Sender<Command>> for Controller {
	fn from(inner: mpsc::Sender<Command>) -> Self {
		Self { inner }
	}
}

#[pin_project]
pub struct Gateway {
	#[pin]
	conn: Connection<Command, Event>,
	finished: bool,
}

impl Gateway {
	async fn connect(
		cred: Credentials,
		resume: bool,
	) -> Result<(Self, Duration, Option<Ready>), Box<dyn Error>> {
		let Credentials {
			guild_id,
			user_id,
			session_id,
			token,
			endpoint,
		} = cred;
		let session_id = session_id.into();
		let token = token.into();

		let url = format!(
			"wss://{}/?v={}",
			endpoint.trim_end_matches(":443"),
			API_VERSION
		);
		info!("Connecting to voice gateway v{} ", API_VERSION);

		let codec = Box::new(JsonCodec::new(Some("vgw.log")));
		let (mut conn, _) =
			Connection::<Command, Event>::connect(url, codec, Duration::from_secs(5)).await?;

		// Receive `Hello`
		let hello = read_command!(conn, expect_hello);

		let ready = if resume {
			let command = command::Resume {
				guild_id,
				session_id,
				token,
			};
			// Send `Resume`
			conn.send(command.into()).await?;
			// Receive `Resumed`
			read_command!(conn, expect_resumed);
			None
		} else {
			let command = command::Identify {
				guild_id,
				user_id,
				session_id,
				token,
			};
			// Send `Identify`
			conn.send(command.into()).await?;
			// Receive `Ready`
			Some(read_command!(conn, expect_ready))
		};

		let gateway = Self {
			conn,
			finished: false,
		};

		Ok((
			gateway,
			Duration::from_millis(hello.heartbeat_interval as u64),
			ready,
		))
	}

	pub fn spawn(cred: Credentials, resume: bool) -> (Controller, Listener) {
		debug!("New");
		let (controller, command_recv) = mpsc::channel(16);
		let (event_send, event_recv) = mpsc::channel(16);
		tokio::spawn(async move {
			if let Err(e) = connect(cred, resume, command_recv, event_send).await {
				debug!("Closed: {}", e);
			} else {
				debug!("Closed");
			}
		});

		(controller.into(), event_recv)
	}
}

impl Stream for Gateway {
	type Item = Result<Event, GatewayError>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let project = self.project();
		if *project.finished {
			return Poll::Ready(None);
		}

		let res = project.conn.poll_next(cx);
		match &res {
			Poll::Ready(None) | Poll::Ready(Some(Err(GatewayError::Ws(_)))) => {
				*project.finished = true
			}
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
	type Error = GatewayError;

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

async fn connect(
	cred: Credentials,
	resume: bool,
	command_recv: mpsc::Receiver<Command>,
	mut event_send: mpsc::Sender<Event>,
) -> Result<(), Box<dyn Error>> {
	let (gateway, heartbeat_interval, ready) = Gateway::connect(cred, resume).await?;
	if let Some(ready) = ready {
		debug!("Sending ready event");
		let _ = event_send.send(Event::Ready(ready)).await;
	} else {
		debug!("Sending resume event");
		let _ = event_send.send(Event::Resumed).await;
	}

	let (sink, stream) = gateway.split();
	let reader = read(stream, event_send).fuse();
	let writer = write(sink, command_recv, heartbeat_interval).fuse();
	let (reader, writer) = join(reader, writer).await;
	reader.and(writer)
}

async fn write(
	mut gateway: SplitSink<Gateway, Command>,
	mut command_recv: mpsc::Receiver<Command>,
	heartbeat_interval: Duration,
) -> Result<(), Box<dyn Error>> {
	let mut count = 0u64;
	let mut heartbeat = IntervalStream::new(time::interval_at(
		time::Instant::now() + heartbeat_interval,
		heartbeat_interval,
	))
	.fuse();

	// Write any upstream commands and a periodic heartbeat to the gateway
	loop {
		let command = select! {
			_ = heartbeat.next() => {
				count += 1;
				command::Heartbeat::new(count).into()
			},
			c = command_recv.next() => {
				if let Some(command) = c {
					debug!("Sending {:?}", command);
					command
				}
				else {
					break;
				}
			}
		};
		gateway.send(command).await?;
	}

	gateway.close().await?;
	Ok(())
}

async fn read(
	mut gateway: SplitStream<Gateway>,
	mut event_send: mpsc::Sender<Event>,
) -> Result<(), Box<dyn Error>> {
	while let Some(event) = gateway.next().await {
		let event = match event {
			Ok(event) => event,
			Err(GatewayError::Close(frame)) => return Err(GatewayError::Close(frame).into()),
			Err(e) => {
				warn!("Gateway error: {}", e);
				continue;
			}
		};
		if !event.is_heartbeat_ack() {
			event_send.send(event).await?;
		}
	}

	Ok(())
}
