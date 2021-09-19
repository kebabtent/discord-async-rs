use crate::guild::{Guild, GuildEvent};
use crate::{Client, Connector, Error, Gateway, GatewayError};
use discord_types::event::GuildCreate;
use discord_types::{Application, ApplicationId, Command, Intents, User, UserId};
use futures::channel::mpsc::Receiver;
use futures::channel::{mpsc, oneshot};
use futures::stream::{self, SplitSink, SplitStream};
use futures::{pin_mut, select};
use futures::{Future, FutureExt, SinkExt, Stream, StreamExt};
use log::{debug, warn};
use never::Never;
use pin_project::pin_project;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::{task, time};
use tokio_stream::wrappers::IntervalStream;

type InitSend = oneshot::Sender<Init>;
type Init = (Application, User);

trait Callback<G: Fut>: FnMut(GuildEvent) -> G + Send + Sync + 'static {}
trait Fut: Future<Output = ()> {}

impl<G> Fut for G where G: Future<Output = ()> + Send + Sync + 'static {}

impl<F, G> Callback<G> for F
where
	F: FnMut(GuildEvent) -> G + Send + Sync + 'static,
	G: Fut,
{
}

pub struct Builder<F> {
	token: String,
	intents: Intents,
	callback: F,
}

impl<F> Builder<F> {
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
}

impl<F, G> Builder<F>
where
	F: FnMut(GuildEvent) -> G + Send + Sync + 'static,
	G: Future<Output = ()> + Send + Sync + 'static,
{
	pub fn new(token: String, callback: F) -> Self {
		Self {
			token,
			callback,
			intents: Intents::GUILD_ALL ^ Intents::GUILD_WEBHOOKS ^ Intents::GUILD_MESSAGE_TYPING,
		}
	}

	pub async fn build(self) -> Result<Discord, Error> {
		let (command_send, command_recv) = mpsc::channel(8);
		let (init_send, init_recv) = oneshot::channel();
		let client = Client::new(&self.token, command_send)?;
		task::spawn(start_discord(
			self.token,
			self.intents,
			init_send,
			self.callback,
			command_recv,
		));

		let (application, user) = init_recv
			.await
			.map_err(|_| Error::Gateway(GatewayError::Close(None)))?;

		Ok(Discord {
			application,
			user,
			client,
		})
	}
}

#[pin_project]
pub struct Discord {
	application: Application,
	user: User,
	client: Client,
}

impl Discord {
	pub fn client(&self) -> Client {
		self.client.clone()
	}

	pub fn user_id(&self) -> UserId {
		self.user.id
	}

	pub fn application_id(&self) -> ApplicationId {
		self.application.id
	}

	pub async fn guild<S: Stream<Item = GuildEvent> + Unpin + Sync + Send + 'static>(
		&self,
		stream: S,
		gc: GuildCreate,
	) -> Result<Guild<S>, Error> {
		Guild::new(
			stream,
			gc,
			self.client.clone(),
			self.user.id,
			self.application.id,
		)
		.await
	}
}

async fn start_discord<F: Callback<G>, G: Fut>(
	token: String,
	intents: Intents,
	init_send: InitSend,
	mut callback: F,
	mut command_recv: Receiver<Command>,
) {
	let mut session_id = None;
	let sequence = Arc::new(AtomicU64::new(0));
	let mut init_send = Some(init_send);

	loop {
		match connect(
			&token,
			intents,
			&mut session_id,
			sequence.clone(),
			&mut callback,
			&mut init_send,
			&mut command_recv,
		)
		.await
		{
			Ok(_) => unreachable!(),
			Err(e) => warn!("Connection error: {:?}", e),
		}
		time::sleep(Duration::from_secs(3)).await; // TODO: exponential backoff?
	}
}

async fn connect<F: Callback<G>, G: Fut>(
	token: &str,
	intents: Intents,
	session_id: &mut Option<String>,
	sequence: Arc<AtomicU64>,
	callback: &mut F,
	init_send: &mut Option<InitSend>,
	command_recv: &mut Receiver<Command>,
) -> Result<Never, GatewayError> {
	let connector = match session_id.as_deref() {
		Some(s) => Connector::resume(&token, s, sequence.load(Ordering::Relaxed), intents),
		None => Connector::new(&token, intents),
	};
	let is_new = connector.is_new();

	let (gateway, heartbeat_interval, ready) = connector.connect().await?;
	if let Some(init_send) = init_send.take() {
		match ready {
			Some(ready) => {
				let _ = init_send.send((ready.application, ready.user));
			}
			None => {
				if is_new {
					return Err(GatewayError::UnexpectedEvent);
				}
			}
		}
	}

	callback(GuildEvent::Online).await;

	let res = {
		let (writer, reader) = gateway.split();
		let write_fut = write(writer, command_recv, sequence.clone(), heartbeat_interval).fuse();
		pin_mut!(write_fut);
		let read_fut = read(reader, callback, session_id, sequence).fuse();
		pin_mut!(read_fut);

		select! {
			res = write_fut => res,
			res = read_fut => res,
		}
	};

	callback(GuildEvent::Offline).await;

	res
}

async fn write(
	mut gateway: SplitSink<Gateway, Command>,
	command_recv: &mut Receiver<Command>,
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

async fn read<F: Callback<G>, G: Fut>(
	mut gateway: SplitStream<Gateway>,
	callback: &mut F,
	session_id: &mut Option<String>,
	sequence: Arc<AtomicU64>,
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

		if event.is_invalid_session() {
			// Our session was invalidated, don't try to resume it during our next connection
			warn!("Session invalidated");
			*session_id = None;
			break;
		}

		callback(GuildEvent::Event(event)).await;

		/*use Event::*;
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
				*session_id = None;
				break;
			}
			HeartbeatAck => {
				// TODO: detect dead connection
			}
			Ready(_) | Hello(_) => {
				// Should not be reachable unless server breaks protocol
			}
			Unknown(_) | _ => {}
		}*/
	}

	debug!("Reader shutdown");
	Err(GatewayError::Close(None))
}
