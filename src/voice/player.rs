use super::connection;
use super::connection::{Connection, Event as ConnectionEvent};
use super::opus::OpusStream;
use discord_types::event::{VoiceServerUpdate, VoiceStateUpdate};
use discord_types::{GuildId, UserId};
use futures::channel::mpsc;
use futures::future::Either;
use futures::stream::pending;
use futures::{select, SinkExt, StreamExt};
use log::{debug, warn};

pub type Listener = mpsc::Receiver<Event>;

#[derive(Debug)]
enum Control {
	VoiceServerUpdate(VoiceServerUpdate),
	VoiceStateUpdate(VoiceStateUpdate),
	Play(OpusStream),
	// Pause,
	// Stop,
}

#[derive(Debug)]
pub struct Controller {
	inner: mpsc::Sender<Control>,
}

impl Controller {
	fn try_send(&mut self, c: Control) -> bool {
		if let Err(e) = self.inner.try_send(c) {
			warn!("Controller: {}", e);
			false
		} else {
			true
		}
	}

	pub fn server_update(&mut self, u: VoiceServerUpdate) -> bool {
		self.try_send(Control::VoiceServerUpdate(u))
	}

	pub fn state_update(&mut self, u: VoiceStateUpdate) -> bool {
		self.try_send(Control::VoiceStateUpdate(u))
	}

	pub fn play(&mut self, stream: OpusStream) -> bool {
		self.try_send(Control::Play(stream))
	}
}

impl Clone for Controller {
	fn clone(&self) -> Self {
		Self {
			inner: mpsc::Sender::clone(&self.inner),
		}
	}
}

impl From<mpsc::Sender<Control>> for Controller {
	fn from(inner: mpsc::Sender<Control>) -> Self {
		Self { inner }
	}
}

#[derive(Debug)]
pub enum Event {
	Connected,
	Ready,
	Playing,
	Finished,
	Disconnected,
}

#[derive(Debug)]
pub struct Player {
	guild_id: GuildId,
	user_id: UserId,
	session_id: Option<String>,
	control_recv: mpsc::Receiver<Control>,
	event_send: mpsc::Sender<Event>,
	conn_controller: Option<connection::Controller>,
}

impl Player {
	pub fn new(guild_id: GuildId, user_id: UserId) -> (Self, Controller, Listener) {
		let (controller, control_recv) = mpsc::channel(16);
		let (event_send, event_recv) = mpsc::channel(16);

		let player = Self {
			guild_id,
			user_id,
			session_id: None,
			control_recv,
			event_send,
			conn_controller: None,
		};

		(player, controller.into(), event_recv)
	}

	async fn control(&mut self, control: Control) -> Option<OpusStream> {
		match control {
			Control::VoiceServerUpdate(u) => {
				if let Some(c) = &mut self.conn_controller {
					c.token(u.token).await;
					c.endpoint(u.endpoint).await;
				}
			}
			Control::VoiceStateUpdate(u) => {
				if u.voice_state.user_id != self.user_id {
					return None;
				}

				if self
					.session_id
					.as_ref()
					.map(|id| id != &u.voice_state.session_id)
					.unwrap_or(true)
				{
					let id = u.voice_state.session_id;
					self.session_id = Some(id.clone());
					if let Some(c) = &mut self.conn_controller {
						c.session(id).await;
					}
				}
			}
			Control::Play(s) => return Some(s),
		}
		None
	}

	async fn send_event(&mut self, event: Event) -> bool {
		self.event_send.send(event).await.is_ok()
	}

	async fn event(&mut self, event: ConnectionEvent) -> bool {
		match event {
			ConnectionEvent::Connected => self.send_event(Event::Connected).await,
			ConnectionEvent::Ready => self.send_event(Event::Ready).await,
		}
	}

	pub async fn run(mut self) {
		debug!("Running player");
		let (conn, conn_controller, event_recv) = Connection::new(self.guild_id, self.user_id);
		conn.spawn();
		self.conn_controller = Some(conn_controller);
		let mut event_recv = Either::Left(event_recv);
		let mut enc = Box::pin(Either::Right(pending()));

		loop {
			select! {
				c = self.control_recv.next() => {
					let e = match c {
						Some(ctrl) => self.control(ctrl).await,
						None => break,
					};
					if let Some(e) = e {
						enc = Box::pin(Either::Left(e));
					}
				},
				e = event_recv.next() => {
					let res = match e {
						Some(event) => self.event(event).await,
						None => {
							// Connection closed
							// TODO: reconnect
							event_recv = Either::Right(pending());
							self.conn_controller = None;
							self.send_event(Event::Disconnected).await;
							// TODO
							false
						}
					};
					if !res {
						// Controller went away
						break;
					}
				},
				f = enc.next() => {
					match f {
						Some(Ok(frame)) => {
							// Received a voice frame
							if let Some(c) = &mut self.conn_controller {
								c.voice_frame(frame);
							}
						}
						Some(Err(e)) => {
							warn!("Encoder: {}", e);
						}
						None => {
							// Finished playing
							if !self.send_event(Event::Finished).await {
								break;
							}
							enc = Box::pin(Either::Right(pending()));
						}
					}
				}
			}
		}
	}

	pub fn spawn(self) {
		tokio::spawn(async move {
			self.run().await;
			debug!("Closed");
		});
	}
}
