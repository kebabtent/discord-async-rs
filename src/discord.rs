use crate::guild::{GuildEvent, GuildSeed};
use crate::{client, gateway, protocol};
use crate::{Client, Command, Connector, Gateway, Recv, Send, Snowflake};
use futures::channel::mpsc;
use futures::stream::{self, SplitSink, SplitStream};
use futures::{pin_mut, select};
use futures::{FutureExt, SinkExt, Stream, StreamExt};
use log::{debug, info, warn};
use never::Never;
use pin_project::pin_project;
use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::{task, time};

type Guilds = HashMap<Snowflake, (bool, Option<Send<GuildEvent>>)>;

#[derive(Debug)]
pub enum Error {
	Gateway(gateway::Error),
	Client(client::Error),
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Error::Gateway(e) => write!(f, "Gateway error: {:?}", e),
			Error::Client(e) => write!(f, "Client error: {:?}", e),
		}
	}
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		/*match self {
			Error::Gateway(e) => Some(e),
			Error::Client(e) => Some(e),
		}*/
		None
	}
}

impl From<gateway::Error> for Error {
	fn from(e: gateway::Error) -> Self {
		Error::Gateway(e)
	}
}

impl From<client::Error> for Error {
	fn from(e: client::Error) -> Self {
		Error::Client(e)
	}
}

pub struct Builder {
	token: String,
}

impl Builder {
	pub fn new(token: String) -> Self {
		Self { token }
	}

	pub fn build(self) -> Result<Discord, Error> {
		let (guild_send, guild_recv) = mpsc::channel(8);
		let (command_send, command_recv) = mpsc::channel(8);
		let client = Client::new(self.token.clone())?;
		task::spawn(start_discord(
			self.token,
			guild_send,
			command_recv,
			client.clone(),
		));
		Ok(Discord {
			guild_recv,
			client,
			command_send,
		})
	}
}

#[pin_project]
pub struct Discord {
	#[pin]
	guild_recv: Recv<GuildSeed>,
	client: Client,
	command_send: Send<Command>,
}

impl Discord {
	pub fn client(&self) -> Client {
		self.client.clone()
	}
}

impl Stream for Discord {
	type Item = GuildSeed;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		self.project().guild_recv.poll_next(cx)
	}
}

async fn start_discord(
	token: String,
	mut guild_send: Send<GuildSeed>,
	mut command_recv: Recv<Command>,
	client: Client,
) {
	let mut guilds = HashMap::new();
	let mut session_id: Option<String> = None;
	let sequence = Arc::new(AtomicU64::new(0));

	loop {
		match connect(
			&token,
			&mut session_id,
			sequence.clone(),
			&mut guilds,
			&mut guild_send,
			&mut command_recv,
			client.clone(),
		)
		.await
		{
			Ok(_) => unreachable!(),
			Err(e) => warn!("Connection error: {:?}", e),
		}
		for (_, send) in guilds.values_mut() {
			// Don't mark them offline in our local hashmap so we can deal with resumes
			send_or_drop(send, GuildEvent::Offline);
		}
		time::delay_for(Duration::from_secs(3)).await; // TODO: exponential backoff?
	}
}

async fn connect(
	token: &str,
	session_id: &mut Option<String>,
	sequence: Arc<AtomicU64>,
	guilds: &mut Guilds,
	guild_send: &mut Send<GuildSeed>,
	command_recv: &mut Recv<Command>,
	client: Client,
) -> Result<Never, gateway::Error> {
	let connector = match session_id.as_deref() {
		Some(s) => Connector::resume(&token, s, sequence.load(Ordering::Relaxed)),
		None => Connector::new(&token),
	};

	let (gateway, heartbeat_interval, ready) = connector.connect().await?;
	if let Some(ready) = ready {
		*session_id = Some(ready.session_id);
	}

	let (writer, reader) = gateway.split();
	let write_fut = write(writer, command_recv, sequence.clone(), heartbeat_interval).fuse();
	pin_mut!(write_fut);
	let read_fut = read(reader, guild_send, guilds, session_id, sequence, client).fuse();
	pin_mut!(read_fut);

	select! {
		res = write_fut => res,
		res = read_fut => res,
	}
}

async fn write(
	mut gateway: SplitSink<Gateway, Command>,
	command_recv: &mut Recv<Command>,
	sequence: Arc<AtomicU64>,
	heartbeat_interval: Duration,
) -> Result<Never, gateway::Error> {
	let heartbeat = time::interval_at(
		time::Instant::now() + heartbeat_interval,
		heartbeat_interval,
	)
	.map(|_| Command::heartbeat(sequence.load(Ordering::Relaxed)));

	let mut stream = stream::select(command_recv, heartbeat);

	// Write any upstream commands and a periodic heartbeat to the gateway
	while let Some(item) = stream.next().await {
		gateway.send(item).await?;
	}

	debug!("Writer shutdown");
	Err(gateway::Error::Close(None))
}

async fn read(
	mut gateway: SplitStream<Gateway>,
	guild_send: &mut Send<GuildSeed>,
	guilds: &mut Guilds,
	session_id: &mut Option<String>,
	sequence: Arc<AtomicU64>,
	client: Client,
) -> Result<Never, gateway::Error> {
	while let Some(event) = gateway.next().await {
		let event = match event {
			Ok(p) => {
				if let Some(seq) = p.sequence {
					sequence.store(seq, Ordering::Relaxed);
				}
				p.event
			}
			Err(gateway::Error::Close(frame)) => return Err(gateway::Error::Close(frame)),
			Err(e) => {
				warn!("Gateway error: {:?}", e);
				continue;
			}
		};

		// Dispatch guild events to the appropriate channel
		if let Some(guild_id) = event.guild_id() {
			if let Some((_, send)) = guilds.get_mut(&guild_id) {
				send_or_drop(send, event);
			} else {
				warn!("Message for unknown guild {}", guild_id);
			}
			continue;
		}

		use protocol::Event::*;
		match event {
			GuildCreate(gc) => {
				let guild_id = gc.id;
				let unavailable = gc.unavailable;

				let (available, send) = if let Some((available, send)) = guilds.get_mut(&guild_id) {
					// Existing guild, forward `GuildCreate` event
					send_or_drop(send, gc);
					(available, send)
				} else {
					// 'New' guild (not yet known)
					let (send, recv) = mpsc::channel(16);
					guilds.insert(guild_id, (false, Some(send)));
					let seed = GuildSeed::new(gc, client.clone(), recv);
					let _ = guild_send.try_send(seed);
					let (a, s) = guilds.get_mut(&guild_id).unwrap();
					(a, s)
				};

				if *available == unavailable {
					// Availability toggled, notify guild
					*available = !unavailable;
					let event = if unavailable {
						GuildEvent::Offline
					} else {
						GuildEvent::Online
					};
					send_or_drop(send, event);
				}
			}
			Resumed => {
				// Resume: guilds are online again
				info!("Resumed session");
				for (available, send) in guilds.values_mut() {
					if *available {
						send_or_drop(send, GuildEvent::Online);
					}
				}
			}
			InvalidSession(_) => {
				// Our session was invalidated, don't try to resume it during our next connection
				warn!("Session invalidated, starting a new one..");
				*session_id = None;
			}
			HearbeatAck => {
				// TODO: detect dead connection
			}
			Unknown(_) => {}
			Hello(_) | Ready(_) => {}
			_ => {}
		}
	}

	debug!("Reader shutdown");
	Err(gateway::Error::Close(None))
}

fn send_or_drop<T, U: Into<T>>(send: &mut Option<Send<T>>, item: U) {
	if let Some(s) = send {
		match s.try_send(item.into()) {
			Err(e) if e.is_disconnected() => {
				debug!("Channel closed");
				*send = None;
			}
			Err(e) if e.is_full() => warn!("Channel full"),
			_ => {}
		}
	}
}
