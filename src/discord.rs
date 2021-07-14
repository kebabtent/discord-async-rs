use crate::guild::{GuildEvent, GuildSeed};
use crate::{Client, Connector, Error, Gateway, GatewayError, Guild, ProtocolError, Recv, Send};
use discord_types::{Application, ApplicationId, Command, Event, GuildId, Intents, User, UserId};
use futures::channel::mpsc;
use futures::stream::{self, SplitSink, SplitStream};
use futures::{pin_mut, select};
use futures::{FutureExt, SinkExt, Stream, StreamExt};
use log::{debug, info, warn};
use never::Never;
use pin_project::pin_project;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::{task, time};
use tokio_stream::wrappers::IntervalStream;

type Guilds = HashMap<GuildId, (bool, Option<Send<GuildEvent>>)>;

pub struct Builder {
	token: String,
	intents: Intents,
}

impl Builder {
	pub fn new(token: String) -> Self {
		Self {
			token,
			intents: Intents::GUILD_ALL ^ Intents::GUILD_WEBHOOKS ^ Intents::GUILD_MESSAGE_TYPING,
		}
	}

	pub fn with_webhooks(mut self) -> Self {
		self.intents |= Intents::GUILD_WEBHOOKS;
		self
	}

	pub fn without_presences(mut self) -> Self {
		self.intents ^= Intents::GUILD_PRESENCES;
		self
	}

	pub fn with_typing(mut self) -> Self {
		self.intents |= Intents::GUILD_MESSAGE_TYPING;
		self
	}

	pub fn with(mut self, intents: Intents) -> Self {
		self.intents |= intents;
		self
	}

	pub fn without(mut self, intents: Intents) -> Self {
		self.intents ^= intents;
		self
	}

	pub fn build(self) -> Result<Discord, Error> {
		let (seed_send, seed_recv) = mpsc::channel(8);
		let (command_send, command_recv) = mpsc::channel(8);
		let client = Client::new(self.token.clone(), command_send.clone())?;
		task::spawn(start_discord(
			self.token,
			self.intents,
			seed_send,
			command_recv,
			client.clone(),
		));
		Ok(Discord {
			user: None,
			application: None,
			seed_recv,
			client,
			command_send,
		})
	}
}

#[pin_project]
pub struct Discord {
	user: Option<User>,
	application: Option<Application>,
	#[pin]
	seed_recv: Recv<Seed>,
	client: Client,
	command_send: Send<Command>,
}

impl Discord {
	pub fn client(&self) -> Client {
		self.client.clone()
	}

	pub fn user_id(&self) -> Option<UserId> {
		self.user.as_ref().map(|a| a.id)
	}

	pub fn application_id(&self) -> Option<ApplicationId> {
		self.application.as_ref().map(|a| a.id)
	}

	pub async fn seed_guild(&self, seed: GuildSeed) -> Result<Guild, Error> {
		Guild::new(
			seed,
			self.user_id()
				.ok_or_else(|| ProtocolError::MissingField("user"))?,
			self.application_id()
				.ok_or_else(|| ProtocolError::MissingField("application"))?,
		)
		.await
	}
}

impl Stream for Discord {
	type Item = GuildSeed;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let mut this = self.as_mut().project();
		loop {
			return match this.seed_recv.as_mut().poll_next(cx) {
				Poll::Ready(Some(Seed::Guild(guild))) => Poll::Ready(Some(guild)),
				Poll::Ready(Some(Seed::User(user))) => {
					if this.user.is_none() {
						info!("User id: {}", user.id);
						*this.user = Some(user);
					}
					continue;
				}
				Poll::Ready(Some(Seed::Application(application))) => {
					if this.application.is_none() {
						info!("Application id: {}", application.id);
						*this.application = Some(application);
					}
					continue;
				}
				Poll::Ready(None) => Poll::Ready(None),
				Poll::Pending => Poll::Pending,
			};
		}
	}
}

enum Seed {
	User(User),
	Application(Application),
	Guild(GuildSeed),
}

struct Meta {
	pub session_id: Option<String>,
	pub user: Option<User>,
	pub application: Option<Application>,
}

impl Meta {
	pub fn new() -> Self {
		Self {
			session_id: None,
			user: None,
			application: None,
		}
	}
}

async fn start_discord(
	token: String,
	intents: Intents,
	mut seed_send: Send<Seed>,
	mut command_recv: Recv<Command>,
	client: Client,
) {
	let mut guilds = HashMap::new();
	let mut meta = Meta::new();
	let sequence = Arc::new(AtomicU64::new(0));

	loop {
		match connect(
			&token,
			intents,
			&mut meta,
			sequence.clone(),
			&mut guilds,
			&mut seed_send,
			&mut command_recv,
			client.clone(),
		)
		.await
		{
			Ok(_) => unreachable!(),
			Err(e) => warn!("Connection error: {:?}", e),
		}
		for (_, send) in guilds.values_mut() {
			// Don't mark them offline in our local map so we can deal with resumes
			send_or_drop(send, GuildEvent::Offline);
		}
		time::sleep(Duration::from_secs(3)).await; // TODO: exponential backoff?
	}
}

async fn connect(
	token: &str,
	intents: Intents,
	meta: &mut Meta,
	sequence: Arc<AtomicU64>,
	guilds: &mut Guilds,
	seed_send: &mut Send<Seed>,
	command_recv: &mut Recv<Command>,
	client: Client,
) -> Result<Never, GatewayError> {
	let connector = match meta.session_id.as_deref() {
		Some(s) => Connector::resume(&token, s, sequence.load(Ordering::Relaxed), intents),
		None => Connector::new(&token, intents),
	};

	let (gateway, heartbeat_interval, ready) = connector.connect().await?;
	if let Some(ready) = ready {
		meta.session_id = Some(ready.session_id);
		meta.user = Some(ready.user.clone());
		meta.application = Some(ready.application);

		let _ = seed_send.send(Seed::User(ready.user)).await;
		let _ = seed_send.send(Seed::Application(ready.application)).await;
	}

	let (writer, reader) = gateway.split();
	let write_fut = write(writer, command_recv, sequence.clone(), heartbeat_interval).fuse();
	pin_mut!(write_fut);
	let read_fut = read(reader, seed_send, guilds, meta, sequence, client).fuse();
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
) -> Result<Never, GatewayError> {
	let heartbeat = IntervalStream::new(time::interval_at(
		time::Instant::now() + heartbeat_interval,
		heartbeat_interval,
	))
	.map(|_| Command::heartbeat(sequence.load(Ordering::Relaxed)));

	let mut stream = stream::select(command_recv, heartbeat);

	// Write any upstream commands and a periodic heartbeat to the gateway
	while let Some(item) = stream.next().await {
		gateway.send(item).await?;
	}
	gateway.close().await?;

	debug!("Writer shutdown");
	Err(GatewayError::Close(None))
}

async fn read(
	mut gateway: SplitStream<Gateway>,
	seed_send: &mut Send<Seed>,
	guilds: &mut Guilds,
	meta: &mut Meta,
	sequence: Arc<AtomicU64>,
	client: Client,
) -> Result<Never, GatewayError> {
	while let Some(event) = gateway.next().await {
		let event = match event {
			Ok(p) => {
				if let Some(seq) = p.sequence {
					sequence.store(seq, Ordering::Relaxed);
				}
				p.event
			}
			Err(GatewayError::Close(frame)) => return Err(GatewayError::Close(frame)),
			Err(e) => {
				warn!("Gateway error: {}", e);
				continue;
			}
		};

		use Event::*;
		if let GuildCreate(gc) = &event {
			if meta.application.is_none() {
				warn!("Received guild before being ready");
				break;
			}

			let guild_id = gc.guild.id;
			let unavailable = gc.guild.unavailable.unwrap_or(true);

			if !guilds.contains_key(&guild_id) {
				// 'New' guild (not yet known)
				let (send, recv) = mpsc::channel(16);
				guilds.insert(guild_id, (false, Some(send)));
				let seed = GuildSeed::new(gc.clone(), client.clone(), recv);
				let _ = seed_send.try_send(Seed::Guild(seed));
			}

			let (available, send) = guilds.get_mut(&guild_id).unwrap();
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

		// Dispatch guild events to the appropriate channel
		if let Some(guild_id) = event.guild_id() {
			if let Some((_, send)) = guilds.get_mut(&guild_id) {
				send_or_drop(send, event);
			} else {
				warn!("Message for unknown guild {}", guild_id);
			}
			continue;
		}

		match event {
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
				meta.session_id = None;
				break;
			}
			HeartbeatAck => {
				// TODO: detect dead connection
			}
			Ready(_) | Hello(_) => {
				// Should not be reachable unless server breaks protocol
			}
			Unknown(_) | _ => {}
		}
	}

	debug!("Reader shutdown");
	Err(GatewayError::Close(None))
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
