use super::gateway::{ConnectParams, Gateway};
use super::opus::OpusFrame;
use super::socket::Socket;
use super::{gateway, socket, Error};
use discord_types::voice::{Event as GatewayEvent, Ready};
use discord_types::{GuildId, UserId};
use futures::channel::mpsc;
use futures::future::Either;
use futures::stream::pending;
use futures::{select, SinkExt, StreamExt};
use log::{debug, warn};
use std::net::SocketAddr;

pub type Listener = mpsc::Receiver<Event>;
const ENCRYPTION_MODE: &'static str = "xsalsa20_poly1305";

#[derive(Debug)]
enum Control {
	Session(String),
	Token(String),
	Endpoint(Option<String>),
	VoiceFrame(OpusFrame),
}

impl Control {
	fn is_voice_frame(&self) -> bool {
		match self {
			Control::VoiceFrame(_) => true,
			_ => false,
		}
	}
}

#[derive(Debug)]
pub struct Controller {
	inner: mpsc::Sender<Control>,
}

impl Controller {
	async fn send(&mut self, control: Control) -> bool {
		if let Err(e) = self.inner.send(control).await {
			warn!("Controller: {}", e);
			false
		} else {
			true
		}
	}

	pub async fn session(&mut self, session: String) -> bool {
		self.send(Control::Session(session)).await
	}

	pub async fn token(&mut self, token: String) -> bool {
		self.send(Control::Token(token)).await
	}

	pub async fn endpoint(&mut self, endpoint: Option<String>) -> bool {
		self.send(Control::Endpoint(endpoint)).await
	}

	pub fn voice_frame(&mut self, frame: OpusFrame) -> bool {
		self.inner.try_send(Control::VoiceFrame(frame)).is_ok()
	}
}

impl From<mpsc::Sender<Control>> for Controller {
	fn from(inner: mpsc::Sender<Control>) -> Self {
		Self { inner }
	}
}

pub enum Event {
	Connected,
	Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
	Idle,
	Connecting,
	Connected,
}

pub struct Connection {
	guild_id: GuildId,
	user_id: UserId,
	state: State,
	session_id: Option<String>,
	token: Option<String>,
	endpoint: Option<String>,
	ready: Option<Ready>,
	external_addr: Option<SocketAddr>,
	control_recv: mpsc::Receiver<Control>,
	event_send: mpsc::Sender<Event>,
	gw_controller: Option<gateway::Controller>,
	sock_controller: Option<socket::Controller>,
}

impl Connection {
	pub fn new(guild_id: GuildId, user_id: UserId) -> (Self, Controller, Listener) {
		let (control_send, control_recv) = mpsc::channel(16);
		let (event_send, event_recv) = mpsc::channel(16);

		let connection = Self {
			guild_id,
			user_id,
			state: State::Idle,
			session_id: None,
			token: None,
			endpoint: None,
			ready: None,
			external_addr: None,
			control_recv,
			event_send,
			gw_controller: None,
			sock_controller: None,
		};

		(connection, control_send.into(), event_recv)
	}

	fn connect_params(&self) -> Option<ConnectParams> {
		Some(ConnectParams {
			guild_id: self.guild_id,
			user_id: self.user_id,
			session_id: self.session_id.as_ref()?.clone().into(),
			token: self.token.as_ref()?.clone().into(),
			endpoint: self.endpoint.as_ref()?.clone().into(),
		})
	}

	fn ssrc(&self) -> u32 {
		self.ready.as_ref().map(|r| r.ssrc).unwrap_or(0)
	}

	/// Called when we receive a command from the controller
	async fn control(&mut self, control: Control) {
		if !control.is_voice_frame() {
			debug!("Control: {:?}", control);
		}
		match control {
			Control::Session(session) => self.session_id = Some(session),
			Control::Token(token) => self.token = Some(token),
			Control::Endpoint(endpoint) => self.endpoint = endpoint,
			Control::VoiceFrame(frame) => {
				if self.external_addr.is_none() {
					// Socket not yet ready to send frames
					return;
				}

				if let Some(c) = &mut self.sock_controller {
					c.voice_frame(frame);
				}
			}
		}
	}

	/// Called when we receive an event from the underlying gateway connection
	async fn event(&mut self, event: GatewayEvent) -> bool {
		debug!("Event: {:?}", event);
		let ready_kind = event.is_ready_kind();
		match event {
			GatewayEvent::Ready(ready) => {
				if !ready.modes.iter().any(|x| x == ENCRYPTION_MODE) {
					warn!("Encryption mode '{}' not supported", ENCRYPTION_MODE);
					return false;
				}
				self.ready = Some(ready);
			}
			GatewayEvent::SessionDescription(desc) => {
				if let Some(c) = &mut self.sock_controller {
					if desc.mode != ENCRYPTION_MODE {
						warn!("Unexpected encryption mode '{}'", desc.mode);
						return false;
					}
					c.secret_key(desc.secret_key).await;
					let _ = self.event_send.send(Event::Ready).await;
				}
			}
			_ => {}
		}

		if self.state == State::Connecting && ready_kind && self.ready.is_some() {
			let _ = self.event_send.send(Event::Connected).await;
			self.state = State::Connected;
		}
		true
	}

	async fn socket_event(&mut self, event: socket::Event) {
		use socket::Event::*;
		match event {
			AddrDiscovery(addr) => {
				if self.external_addr.is_none() {
					debug!("Our remote address is {}", addr);
					self.external_addr = Some(addr);
					let ssrc = self.ssrc();
					if let Some(c) = &mut self.gw_controller {
						c.select_protocol(addr, ENCRYPTION_MODE).await;
						c.speaking(ssrc, false).await;
					}
				}
			}
			VoiceFrame(_) => {
				// Discard received voice frames
			}
		}
	}

	pub async fn run(mut self) -> Result<(), Error> {
		let mut gw_event_recv = Either::Right(pending());
		let mut sock_event_recv = Either::Right(pending());

		loop {
			select! {
				c = self.control_recv.next() => match c {
					Some(ctrl) => self.control(ctrl).await,
					None => {
						debug!("Controller shutdown");
						break;
					}
				},
				e = gw_event_recv.next() => match e {
					Some(event) => if !self.event(event).await {
						break;
					}
					None => {
						debug!("Gateway shutdown");
						break;
					}
				},
				e = sock_event_recv.next() => match e {
					Some(event) => self.socket_event(event).await,
					None => {
						debug!("Socket shutdown");
						break;
					},
				}
			}

			match self.state {
				State::Idle => {
					if let Some(params) = self.connect_params() {
						let (controller, event_recv) = Gateway::spawn(params, self.ready.is_some());
						self.gw_controller = Some(controller);
						gw_event_recv = Either::Left(event_recv);
						self.state = State::Connecting;
					}
				}
				State::Connected => {
					if self.sock_controller.is_none() {
						debug!("Opening socket");
						let ready = self.ready.as_ref().unwrap();
						let (sock, controller, event_recv) =
							Socket::new(format!("{}:{}", ready.ip, ready.port), ready.ssrc)
								.unwrap();
						// TODO: get rid of unwrap
						sock.spawn();
						self.sock_controller = Some(controller);
						sock_event_recv = Either::Left(event_recv);
					}
				}
				_ => {}
			}
		}
		Ok(())
	}

	pub fn spawn(self) {
		tokio::spawn(async move {
			if let Err(e) = self.run().await {
				debug!("Closed: {}", e);
			} else {
				debug!("Closed");
			}
		});
	}
}
