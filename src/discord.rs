use crate::guild::{GuildEvent, GuildSeed};
use crate::protocol;
use crate::{Command, Connector, Error, Gateway, Snowflake};
use futures::channel::mpsc;
use futures::stream::{self, SplitSink, SplitStream};
use futures::{pin_mut, select};
use futures::{FutureExt, SinkExt, Stream, StreamExt};
use log::{debug, warn};
use never::Never;
use pin_project::pin_project;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::{task, time};

// TODO: bounded channels
type Send<T> = mpsc::UnboundedSender<T>;
pub type Recv<T> = mpsc::UnboundedReceiver<T>;

type Guilds = HashMap<Snowflake, (bool, Send<GuildEvent>)>;

pub struct Builder {
	token: String,
}

impl Builder {
	pub fn new(token: String) -> Self {
		Self { token }
	}

	pub fn build(self) -> Discord {
		let (guild_send, guild_recv) = mpsc::unbounded();
		let (command_send, command_recv) = mpsc::unbounded();
		task::spawn(start_discord(self.token, guild_send, command_recv));
		Discord {
			guild_recv,
			command_send,
		}
	}
}

#[pin_project]
pub struct Discord {
	#[pin]
	guild_recv: Recv<GuildSeed>,
	command_send: Send<Command>,
}

impl Discord {
	pub fn client(&self) {}
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
		)
		.await
		{
			Ok(_) => unreachable!(),
			Err(e) => warn!("Connection error: {:?}", e),
		}
		for (_, event_send) in guilds.values_mut() {
			// Don't mark them offline in our local hashmap so we can deal with resumes
			let _ = event_send.unbounded_send(GuildEvent::Offline);
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
) -> Result<Never, Error> {
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
	let read_fut = read(reader, guild_send, guilds, session_id, sequence).fuse();
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
) -> Result<Never, Error> {
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
	Err(Error::Close(None))
}

async fn read(
	mut gateway: SplitStream<Gateway>,
	guild_send: &mut Send<GuildSeed>,
	guilds: &mut Guilds,
	session_id: &mut Option<String>,
	sequence: Arc<AtomicU64>,
) -> Result<Never, Error> {
	while let Some(event) = gateway.next().await {
		let event = match event {
			Ok(p) => {
				if let Some(seq) = p.sequence {
					sequence.store(seq, Ordering::Relaxed);
				}
				p.event
			}
			Err(Error::Close(frame)) => return Err(Error::Close(frame)),
			Err(e) => {
				warn!("Gateway error: {:?}", e);
				continue;
			}
		};

		// Dispatch guild events to the appropriate channel
		if let Some(guild_id) = event.guild_id() {
			if let Some((_, send)) = guilds.get_mut(&guild_id) {
				let _ = send.unbounded_send(event.into());
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
					let _ = send.unbounded_send(gc.into());
					(available, send)
				} else {
					// 'New' guild (not yet known)
					let (send, recv) = mpsc::unbounded();
					guilds.insert(guild_id, (false, send));
					let seed = GuildSeed::new(gc, recv);
					let _ = guild_send.unbounded_send(seed);
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
					let _ = send.unbounded_send(event);
				}
			}
			Resumed => {
				// Resume: guilds are online again
				for (available, send) in guilds.values_mut() {
					if *available {
						let _ = send.unbounded_send(GuildEvent::Online);
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
	Err(Error::Close(None))
}
