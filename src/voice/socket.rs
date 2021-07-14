use super::opus::OpusFrame;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use futures::channel::mpsc;
use futures::future::select;
use futures::pin_mut;
use futures::{SinkExt, StreamExt};
use log::{debug, warn};
use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;
use xsalsa20poly1305::aead::generic_array::GenericArray;
use xsalsa20poly1305::aead::{AeadInPlace, NewAead};
use xsalsa20poly1305::XSalsa20Poly1305;

pub type Listener = mpsc::Receiver<Event>;

#[derive(Debug)]
enum Control {
	VoiceFrame(OpusFrame),
	SecretKey([u8; 32]),
}

#[derive(Debug)]
pub struct Controller {
	inner: mpsc::Sender<Control>,
}

impl Controller {
	pub async fn secret_key(&mut self, key: [u8; 32]) -> bool {
		if let Err(e) = self.inner.send(Control::SecretKey(key)).await {
			warn!("Controller: {}", e);
			false
		} else {
			true
		}
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

#[derive(Clone, Debug)]
pub enum Event {
	AddrDiscovery(SocketAddr),
	VoiceFrame(usize),
}

pub struct Socket {
	addr: SocketAddr,
	ssrc: u32,
	control_recv: mpsc::Receiver<Control>,
	event_send: mpsc::Sender<Event>,
}

impl Socket {
	pub fn new<T: ToSocketAddrs>(
		addr: T,
		ssrc: u32,
	) -> std::io::Result<(Self, Controller, Listener)> {
		let (controller, control_recv) = mpsc::channel(16);
		let (event_send, event_recv) = mpsc::channel(16);
		let addr = addr
			.to_socket_addrs()?
			.next()
			.ok_or_else(|| std::io::ErrorKind::UnexpectedEof)?;

		let socket = Self {
			addr,
			ssrc,
			control_recv,
			event_send,
		};
		Ok((socket, controller.into(), event_recv))
	}

	async fn run(mut self) -> Result<(), Box<dyn Error>> {
		let sock = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);

		let mut buf = [0; 128];
		let mut cursor = &mut buf[..];
		cursor.write_u16::<BigEndian>(1)?;
		cursor.write_u16::<BigEndian>(70)?;
		cursor.write_u32::<BigEndian>(self.ssrc)?;

		let len = sock.send_to(&buf[..74], self.addr).await?;
		debug_assert_eq!(len, 74, "ip discovery packet length");
		let (len, addr) = timeout(Duration::from_secs(10), sock.recv_from(&mut buf[..])).await??;
		if addr != self.addr {
			return Err(format!("Packet from unknown peer {}", addr).into());
		}
		let ty = (&buf[..2]).read_u16::<BigEndian>()?;
		if ty != 2 {
			return Err(format!("Unexpected packet type {}", ty).into());
		}
		if len < 74 {
			return Err("Unexpected packet".into());
		}

		let ip = buf[8..72]
			.split(|&v| v == 0)
			.next()
			.ok_or_else(|| "Invalid ip")?;
		let ip = std::str::from_utf8(ip)?;
		let port = (&buf[72..74]).read_u16::<BigEndian>()?;
		let addr = format!("{}:{}", ip, port).parse()?;
		self.event_send.send(Event::AddrDiscovery(addr)).await?;

		let writer = write(Arc::clone(&sock), self.ssrc, self.addr, self.control_recv);
		pin_mut!(writer);
		let reader = read(sock, self.event_send);
		pin_mut!(reader);

		select(reader, writer).await.factor_first().0
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

async fn read(
	sock: Arc<UdpSocket>,
	mut event_send: mpsc::Sender<Event>,
) -> Result<(), Box<dyn Error>> {
	let mut buf = [0u8; 512];
	loop {
		let (len, _) = sock.recv_from(&mut buf).await?;
		event_send.send(Event::VoiceFrame(len)).await?;
	}
}

async fn write(
	sock: Arc<UdpSocket>,
	ssrc: u32,
	addr: SocketAddr,
	mut control_recv: mpsc::Receiver<Control>,
) -> Result<(), Box<dyn Error>> {
	let mut sequence = 0u16;
	let mut timestamp = 0u32;
	let mut cipher = None;
	let mut buf = Vec::<u8>::with_capacity(512);
	let mut nonce = [0u8; 24];
	let mut packet = [0u8; 512];
	packet[0] = 0x80;
	packet[1] = 0x78;
	(&mut packet[8..12]).write_u32::<BigEndian>(ssrc)?;

	while let Some(control) = control_recv.next().await {
		match control {
			Control::SecretKey(key) => {
				// cipher = Some(Key::from_slice(&key).ok_or_else(|| "Invalid encryption key")?);
				cipher = Some(
					XSalsa20Poly1305::new_from_slice(&key).map_err(|_| "Invalid encryption key")?,
				);
			}
			Control::VoiceFrame(frame) => {
				if let Some(c) = &cipher {
					// Copy data into buffer
					buf.resize(frame.len(), 0);
					buf.copy_from_slice(&frame);

					// Prepare packet
					(&mut packet[2..4]).write_u16::<BigEndian>(sequence)?;
					(&mut packet[4..8]).write_u32::<BigEndian>(timestamp)?;

					// Prepare nonce
					nonce[..12].copy_from_slice(&packet[..12]);
					let n = GenericArray::from_slice(&nonce);

					// Encrypt
					c.encrypt_in_place(n, &[], &mut buf)
						.map_err(|_| "Encryption error")?;
					let len = buf.len() + 12;

					// Copy encrypted data into packet
					// TODO: use `encrypt_in_place_detached` to save some copying
					packet[12..len].copy_from_slice(&buf);
					let written = sock.send_to(&packet[..len], addr).await?;
					debug_assert_eq!(written, len);

					sequence = sequence.wrapping_add(1);
					timestamp = timestamp.wrapping_add(960);
				}
			}
		}
	}
	Ok(())
}
