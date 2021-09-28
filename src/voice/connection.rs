pub use super::gateway::Credentials;
use super::gateway::Gateway;
use super::socket::Socket;
pub use super::socket::VoiceFrameSender;
use super::{gateway, socket, Error};
use async_fuse::Fuse;
use discord_types::voice::{Event as GatewayEvent, Ready};
use futures::channel::mpsc;
use futures::SinkExt;
use log::{debug, warn};
use std::net::SocketAddr;
use tokio::select;

pub type Listener = mpsc::Receiver<Event>;
const ENCRYPTION_MODE: &'static str = "xsalsa20_poly1305";

#[derive(Debug)]
pub enum Event {
	Connected,
	VoiceFrameSender(VoiceFrameSender),
}

impl From<VoiceFrameSender> for Event {
	fn from(voice_send: VoiceFrameSender) -> Self {
		Event::VoiceFrameSender(voice_send)
	}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
	Idle,
	Connecting,
	Connected,
}

pub struct Connection {
	state: State,
	cred: Credentials,
	ready: Option<Ready>,
	external_addr: Option<SocketAddr>,
	event_send: mpsc::Sender<Event>,
	gw_controller: Option<gateway::Controller>,
	key_send: Option<socket::KeySender>,
}

impl Connection {
	pub fn new(cred: Credentials) -> (Self, Listener) {
		debug!("New");
		let (event_send, event_recv) = mpsc::channel(16);

		let connection = Self {
			state: State::Idle,
			cred,
			ready: None,
			external_addr: None,
			event_send,
			gw_controller: None,
			key_send: None,
		};

		(connection, event_recv)
	}

	fn ssrc(&self) -> u32 {
		self.ready.as_ref().map(|r| r.ssrc).unwrap_or(0)
	}

	/// Called when we receive an event from the underlying gateway connection
	async fn event(&mut self, event: GatewayEvent) -> bool {
		debug!("GW event: {:?}", event);
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
				if let Some(sender) = self.key_send.take() {
					if desc.mode != ENCRYPTION_MODE {
						warn!("Unexpected encryption mode '{}'", desc.mode);
						return false;
					}
					if sender.send(desc.secret_key).is_err() {
						return false;
					}
					// let _ = self.event_send.send(Event::Ready).await;
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
		if !event.is_voice_frame() {
			debug!("Socket event: {:?}", event);
		}

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
			VoiceFrameSender(voice_send) => {
				let _ = self.event_send.send(voice_send.into()).await;
			}
			VoiceFrame(_) => {
				// Discard received voice frames
			}
		}
	}

	pub async fn run(mut self) -> Result<(), Error> {
		let (controller, event_recv) = Gateway::spawn(self.cred.clone(), self.ready.is_some());
		self.state = State::Connecting;
		self.gw_controller = Some(controller);

		let mut gw_event_recv = Fuse::new(event_recv);
		let mut sock_event_recv = Fuse::empty();

		loop {
			select! {
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
				State::Connected => {
					if sock_event_recv.is_empty() {
						debug!("Opening socket");
						let ready = match &self.ready {
							Some(r) => r,
							None => {
								warn!("Not ready");
								break;
							}
						};
						let (sock, key_send, event_recv) =
							match Socket::new(format!("{}:{}", ready.ip, ready.port), ready.ssrc) {
								Ok(s) => s,
								Err(e) => {
									warn!("Unable to start udp connection: {}", e);
									break;
								}
							};
						sock.spawn();
						self.key_send = Some(key_send);
						sock_event_recv.set(event_recv);
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
