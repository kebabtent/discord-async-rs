use super::connection;
use super::connection::{Connection, Credentials, Event as ConnectionEvent, VoiceFrameSender};
use super::opus::OpusStream;
use crate::Client;
use async_fuse::Fuse;
use discord_types::event::{VoiceServerUpdate, VoiceStateUpdate};
use discord_types::{ChannelId, GuildId, UserId};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use log::{debug, warn};
use std::pin::Pin;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::{select, time};

pub type Listener = mpsc::Receiver<Event>;

macro_rules! item {
	($v:ident) => {
		match $v {
			Some(v) => v,
			None => break,
		}
	};
}

#[derive(Debug)]
enum Control {
	Connect(ChannelId),
	Disconnect,
	Shutdown,
	Play(OpusStream),
	Stop,
}

#[derive(Debug)]
pub struct Controller {
	inner: mpsc::Sender<Control>,
}

impl Controller {
	fn create() -> (Self, mpsc::Receiver<Control>) {
		let (inner, recv) = mpsc::channel(16);
		let controller = Self { inner };
		(controller, recv)
	}

	fn try_send(&mut self, c: Control) -> bool {
		if let Err(e) = self.inner.try_send(c) {
			warn!("Controller: {}", e);
			false
		} else {
			true
		}
	}

	pub fn connect(&mut self, channel_id: ChannelId) -> bool {
		self.try_send(Control::Connect(channel_id))
	}

	pub fn disconnect(&mut self) -> bool {
		self.try_send(Control::Disconnect)
	}

	pub fn shutdown(&mut self) -> bool {
		self.try_send(Control::Shutdown)
	}

	pub fn play(&mut self, stream: OpusStream) -> bool {
		self.try_send(Control::Play(stream))
	}

	pub fn stop(&mut self) -> bool {
		self.try_send(Control::Stop)
	}
}

impl Clone for Controller {
	fn clone(&self) -> Self {
		Self {
			inner: mpsc::Sender::clone(&self.inner),
		}
	}
}

#[derive(Debug)]
enum Update {
	GuildOnline,
	GuildOffline,
	SessionInvalidated,
	VoiceServerUpdate(VoiceServerUpdate),
	VoiceStateUpdate(VoiceStateUpdate),
}

#[derive(Debug)]
pub struct Updater {
	inner: mpsc::Sender<Update>,
}

impl Updater {
	fn create() -> (Self, mpsc::Receiver<Update>) {
		let (inner, recv) = mpsc::channel(16);
		let updater = Self { inner };
		(updater, recv)
	}

	fn try_send(&mut self, u: Update) -> bool {
		if let Err(e) = self.inner.try_send(u) {
			warn!("Updater: {}", e);
			false
		} else {
			true
		}
	}

	pub fn guild_online(&mut self) -> bool {
		self.try_send(Update::GuildOnline)
	}

	pub fn guild_offline(&mut self) -> bool {
		self.try_send(Update::GuildOffline)
	}

	pub fn session_invalidated(&mut self) -> bool {
		self.try_send(Update::SessionInvalidated)
	}

	pub fn server_update(&mut self, u: VoiceServerUpdate) -> bool {
		self.try_send(Update::VoiceServerUpdate(u))
	}

	pub fn state_update(&mut self, u: VoiceStateUpdate) -> bool {
		self.try_send(Update::VoiceStateUpdate(u))
	}
}

#[derive(Debug)]
pub enum Event {
	Connected(ChannelId),
	ConnectError,
	Playing,
	Stopped(OpusStream),
	Finished,
	Disconnected(Option<OpusStream>),
	Reconnecting(Option<OpusStream>),
}

#[derive(Debug)]
enum State {
	Idle,
	Connecting,
	Connected,
	Shutdown,
}

impl State {
	fn is_connecting(&self) -> bool {
		match self {
			State::Connecting => true,
			_ => false,
		}
	}

	fn is_connected(&self) -> bool {
		match self {
			State::Connected => true,
			_ => false,
		}
	}

	fn is_shutdown(&self) -> bool {
		match self {
			State::Shutdown => true,
			_ => false,
		}
	}
}

pub struct Player {
	guild_id: GuildId,
	user_id: UserId,
	client: Client,
	state: State,
	channel_id: Option<ChannelId>,
	session_id: Option<String>,
	token: Option<String>,
	endpoint: Option<String>,
	update_recv: mpsc::Receiver<Update>,
	control_recv: mpsc::Receiver<Control>,
	event_send: mpsc::Sender<Event>,
	conn_event_recv: Fuse<connection::Listener>,
	connect_timeout: Fuse<Pin<Box<time::Sleep>>>,
	voice_send: Option<VoiceFrameSender>,
	encoder: Fuse<OpusStream>,
}

impl Player {
	pub fn new(
		guild_id: GuildId,
		user_id: UserId,
		client: Client,
	) -> (Self, Updater, Controller, Listener) {
		let (controller, control_recv) = Controller::create();
		let (updater, update_recv) = Updater::create();
		let (event_send, event_recv) = mpsc::channel(16);

		let player = Self {
			guild_id,
			user_id,
			client,
			state: State::Idle,
			channel_id: None,
			session_id: None,
			token: None,
			endpoint: None,
			update_recv,
			control_recv,
			event_send,
			conn_event_recv: Fuse::empty(),
			connect_timeout: Fuse::empty(),
			voice_send: None,
			encoder: Fuse::empty(),
		};

		(player, updater, controller, event_recv)
	}

	async fn emit_event(&mut self, event: Event) -> bool {
		self.event_send.send(event).await.is_ok()
	}

	fn server_credentials(&self) -> Option<Credentials> {
		Some(Credentials {
			guild_id: self.guild_id,
			user_id: self.user_id,
			session_id: self.session_id.as_ref()?.clone(),
			token: self.token.as_ref()?.clone(),
			endpoint: self.endpoint.as_ref()?.clone(),
		})
	}

	fn update_voice_state(&mut self, channel_id: Option<ChannelId>) -> bool {
		self.client
			.update_voice_state(self.guild_id, channel_id, false, false)
			.is_ok()
	}

	async fn clear_connection(&mut self, reconnect: bool) {
		let stream = Fuse::take(&mut self.encoder);
		if self.state.is_connected() {
			if reconnect {
				self.emit_event(Event::Reconnecting(stream)).await;
			} else {
				self.emit_event(Event::Disconnected(stream)).await;
			}
		}

		if reconnect {
			self.state = State::Connecting;
			self.connect_timeout
				.set(Box::pin(time::sleep(Duration::from_secs(5))));
		} else {
			self.state = State::Idle;
		}

		self.channel_id = None;
		self.session_id = None;
		self.token = None;
		self.endpoint = None;
		self.conn_event_recv.clear();
		self.voice_send = None;
	}

	async fn on_update(&mut self, update: Update) {
		debug!("Update: {:?}", update);
		match update {
			Update::VoiceStateUpdate(u) => {
				let u = u.voice_state;
				if u.user_id != self.user_id || u.guild_id != Some(self.guild_id) {
					// This update isn't about us
					return;
				}

				if !self.state.is_connecting() {
					// We are either idling or already connected to a channel
					// If the update contains a channel_id, it means we are reconnecting or moving
					// If the update doesn't contain a channel_id, it means we got kicked
					self.clear_connection(u.channel_id.is_some()).await;
				}
				self.channel_id = u.channel_id;
				self.session_id = Some(u.session_id);
			}
			Update::VoiceServerUpdate(u) => {
				self.token = Some(u.token);
				self.endpoint = u.endpoint;
				if self.state.is_connected() && self.endpoint.is_none() {
					// We should disconnect from the voice server
					self.clear_connection(false).await;
					self.update_voice_state(None);
				}
			}
			Update::GuildOnline => {
				if self.state.is_connecting() && self.channel_id.is_some() {
					self.update_voice_state(self.channel_id);
				}
			}
			Update::GuildOffline => {
				let channel_id = self.channel_id;
				self.clear_connection(true).await;
				self.channel_id = channel_id;
			}
			_ => {}
		}
		if self.state.is_connecting() && self.conn_event_recv.is_empty() {
			if let Some(cred) = self.server_credentials() {
				debug!("Spawn new connection");
				let (conn, event_recv) = Connection::new(cred);
				conn.spawn();
				self.conn_event_recv.set(event_recv);
			}
		}
	}

	async fn on_control(&mut self, control: Control) {
		debug!("Control: {:?}", control);
		match control {
			Control::Connect(channel_id) => {
				if self.state.is_connected() && self.channel_id == Some(channel_id) {
					// We're already connected to this channel
					self.emit_event(Event::Connected(channel_id)).await;
					return;
				}

				self.clear_connection(true).await;
				self.update_voice_state(Some(channel_id));
			}
			Control::Disconnect => {
				self.clear_connection(false).await;
				self.update_voice_state(None);
			}
			Control::Shutdown => {
				self.clear_connection(false).await;
				self.update_voice_state(None);
				self.state = State::Shutdown;
			}
			Control::Play(stream) => {
				if self.state.is_connected() {
					self.encoder.set(stream);
					self.emit_event(Event::Playing).await;
				} else {
					self.emit_event(Event::Stopped(stream)).await;
				}
			}
			Control::Stop => {
				if let Some(stream) = Fuse::take(&mut self.encoder) {
					self.emit_event(Event::Stopped(stream)).await;
				}
			}
		}
	}

	pub async fn run(mut self) {
		debug!("Running player");
		loop {
			if self.state.is_shutdown() {
				debug!("Shutting down");
				break;
			}

			select! {
				u = self.update_recv.next() => self.on_update(item!(u)).await,
				c = self.control_recv.next() => self.on_control(item!(c)).await,
				e = self.conn_event_recv.next() => {
					debug!("Conn event: {:?}", e);
					match e {
						Some(event) => match event {
							ConnectionEvent::Connected => {},
							ConnectionEvent::VoiceFrameSender(voice_send) => {
								self.voice_send = Some(voice_send);
								self.state = State::Connected;
								self.connect_timeout.clear();
								if let Some(channel_id) = self.channel_id {
									self.emit_event(Event::Connected(channel_id)).await;
								}
							}
						},
						None => {
							// Connection closed
							self.clear_connection(false).await;
						}
					}
				}
				f = self.encoder.next() => {
					// Audio frame from our encoder, forward it to the socket
					match f {
						Some(Ok(frame)) => {
							if let Some(c) = &mut self.voice_send {
								let _ = c.try_send(frame);
							}
						}
						Some(Err(e)) => {
							warn!("Encoder: {}", e);
						}
						None => {
							// Finished playing
							self.encoder.clear();
							self.emit_event(Event::Finished).await;
						}
					}
				}
				_ = &mut self.connect_timeout => {
					if self.state.is_connecting() {
						debug!("Connect timeout");
						self.connect_timeout.clear();
						self.clear_connection(false).await;
						self.update_voice_state(None);
						self.emit_event(Event::ConnectError).await;
					}
				}
			}
		}
	}

	pub fn spawn(self) -> JoinHandle<()> {
		tokio::spawn(async move {
			self.run().await;
			debug!("Closed");
		})
	}
}
