use crate::guild::Guild;
use crate::{Client, Connector, Error, Gateway, GatewayError, GatewayEvent};
use discord_types::event::GuildCreate;
use discord_types::{Application, ApplicationId, Command, Intents, User, UserId};
use futures::channel::mpsc::Receiver;
use futures::channel::{mpsc, oneshot};
use futures::pin_mut;
use futures::stream::{SplitSink, SplitStream};
use futures::{Future, FutureExt, SinkExt, Stream, StreamExt};
use log::{debug, warn};
use never::Never;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::{select, task, time};
use tokio_stream::wrappers::IntervalStream;

type InitSend = oneshot::Sender<Init>;
type Init = (Application, User);
type ShutdownRecv = oneshot::Receiver<()>;

trait Callback<G: Fut>: FnMut(GatewayEvent) -> G + Send + Sync {}
trait Fut: Future<Output = Result<(), GatewayError>> {}

macro_rules! check_shutdown {
	($recv:expr) => {
		match $recv.try_recv() {
			Ok(Some(_)) | Err(_) => break,
			_ => {}
		}
	};
}

impl<G> Fut for G where G: Future<Output = Result<(), GatewayError>> + Send + Sync + 'static {}

impl<F, G> Callback<G> for F
where
	F: FnMut(GatewayEvent) -> G + Send + Sync,
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
	F: FnMut(Option<GatewayEvent>) -> G + Send + Sync + 'static,
	G: Future<Output = Result<(), GatewayError>> + Send + Sync + 'static,
{
	pub fn new(token: String, callback: F) -> Self {
		Self {
			token,
			callback,
			intents: Intents::GUILD_ALL ^ Intents::GUILD_WEBHOOKS ^ Intents::GUILD_MESSAGE_TYPING
				| Intents::MESSAGE_CONTENT,
		}
	}

	pub async fn build(self) -> Result<Discord, Error> {
		let (command_send, command_recv) = mpsc::channel(8);
		let (init_send, init_recv) = oneshot::channel();
		let (shutdown_send, shutdown_recv) = oneshot::channel();
		let client = Client::new(&self.token, Some(command_send))?;
		let handle = task::spawn(start_discord(
			self.token,
			self.intents,
			init_send,
			shutdown_recv,
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
			shutdown: Some(Shutdown(shutdown_send)),
			handle,
		})
	}
}

pub struct Discord {
	application: Application,
	user: User,
	client: Client,
	shutdown: Option<Shutdown>,
	handle: task::JoinHandle<()>,
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

	pub async fn guild<S: Stream<Item = GatewayEvent> + Unpin + Sync + Send + 'static>(
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

	pub fn shutdown(&mut self) -> Option<Shutdown> {
		self.shutdown.take()
	}

	pub fn handle(self) -> task::JoinHandle<()> {
		self.handle
	}
}

pub struct Shutdown(oneshot::Sender<()>);

impl Shutdown {
	pub fn send(self) -> bool {
		self.0.send(()).is_ok()
	}
}

async fn start_discord<F, G>(
	token: String,
	intents: Intents,
	init_send: InitSend,
	mut shutdown_recv: ShutdownRecv,
	mut callback: F,
	mut command_recv: Receiver<Command>,
) where
	F: FnMut(Option<GatewayEvent>) -> G + Send + Sync + 'static,
	G: Future<Output = Result<(), GatewayError>> + Send + Sync + 'static,
{
	let mut session_id = None;
	let sequence = Arc::new(AtomicU64::new(0));
	let mut init_send = Some(init_send);

	loop {
		let mut cb = |ev: GatewayEvent| callback(Some(ev));
		check_shutdown!(shutdown_recv);
		let err = connect(
			&token,
			intents,
			&mut session_id,
			sequence.clone(),
			&mut cb,
			&mut init_send,
			&mut shutdown_recv,
			&mut command_recv,
		)
		.await
		.unwrap_err();
		warn!("Connection error: {:?}", err);
		if err.is_shutdown() {
			break;
		}
		check_shutdown!(shutdown_recv);
		time::sleep(Duration::from_secs(3)).await; // TODO: exponential backoff?
	}
	callback(None);
}

async fn connect<F: Callback<G>, G: Fut>(
	token: &str,
	intents: Intents,
	session_id: &mut Option<String>,
	sequence: Arc<AtomicU64>,
	callback: &mut F,
	init_send: &mut Option<InitSend>,
	shutdown_recv: &mut ShutdownRecv,
	command_recv: &mut Receiver<Command>,
) -> Result<Never, GatewayError> {
	let connector = match session_id.as_deref() {
		Some(s) => Connector::resume(&token, s, sequence.load(Ordering::Relaxed), intents),
		None => Connector::new(&token, intents),
	};
	let is_new = connector.is_new();

	let (gateway, heartbeat_interval, event) = connector.connect().await?;
	if is_new {
		let ready = event.expect_ready()?;
		*session_id = Some(ready.session_id.clone());
		if let Some(init_send) = init_send.take() {
			let _ = init_send.send((ready.application, ready.user));
		}
		callback(GatewayEvent::Online).await?;
	} else {
		if event.is_invalid_session() {
			warn!("Session invalidated");
			*session_id = None;
			callback(GatewayEvent::SessionInvalidated).await?;
			return Err(GatewayError::Close(None));
		}
		callback(GatewayEvent::Online).await?;
		callback(GatewayEvent::Event(event)).await?;
	}

	let (mut writer, mut reader) = gateway.split();
	let res = {
		let write_fut = write(
			&mut writer,
			shutdown_recv,
			command_recv,
			sequence.clone(),
			heartbeat_interval,
		)
		.fuse();
		pin_mut!(write_fut);
		let read_fut = read(&mut reader, callback, sequence).fuse();
		pin_mut!(read_fut);

		select! {
			res = write_fut => res,
			res = read_fut => res,
		}
	};
	let err = res.unwrap_err();
	if err.is_shutdown() {
		let mut gateway = reader.reunite(writer).unwrap();
		let _ = gateway.shutdown().await;
		return Err(err);
	}
	callback(GatewayEvent::Offline).await?;
	Err(err)
}

async fn write(
	gateway: &mut SplitSink<Gateway, Command>,
	mut shutdown_recv: &mut ShutdownRecv,
	command_recv: &mut Receiver<Command>,
	sequence: Arc<AtomicU64>,
	heartbeat_interval: Duration,
) -> Result<Never, GatewayError> {
	let mut shutdown = false;
	let mut heartbeat = IntervalStream::new(time::interval_at(
		time::Instant::now() + heartbeat_interval,
		heartbeat_interval,
	));

	loop {
		let item = select! {
			_ = heartbeat.next() => {
				Command::heartbeat(sequence.load(Ordering::Relaxed))
			}
			item = command_recv.next() => {
				match item {
					Some(c) => c,
					None => break,
				}
			}
			_ = &mut shutdown_recv => {
				command_recv.close();
				shutdown = true;
				continue;
			}
		};
		gateway.send(item).await?;
	}

	debug!("Writer shutdown");
	if shutdown {
		Err(GatewayError::Shutdown)
	} else {
		Err(GatewayError::Close(None))
	}
}

async fn read<F: Callback<G>, G: Fut>(
	gateway: &mut SplitStream<Gateway>,
	callback: &mut F,
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

		callback(GatewayEvent::Event(event)).await?;
	}

	debug!("Reader shutdown");
	Err(GatewayError::Close(None))
}
