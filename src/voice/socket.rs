use super::opus::OpusFrame;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use futures::channel::{mpsc, oneshot};
use futures::future::select;
use futures::pin_mut;
use futures::{SinkExt, StreamExt};
use log::debug;
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
pub type KeySender = oneshot::Sender<SecretKey>;
pub type VoiceFrameSender = mpsc::Sender<OpusFrame>;
type SecretKey = [u8; 32];

#[derive(Clone, Debug)]
pub enum Event {
	AddrDiscovery(SocketAddr),
	VoiceFrameSender(VoiceFrameSender),
	VoiceFrame(usize),
}

impl Event {
	pub fn is_voice_frame(&self) -> bool {
		match self {
			Event::VoiceFrame(_) => true,
			_ => false,
		}
	}
}

pub struct Socket {
	addr: SocketAddr,
	ssrc: u32,
	key_recv: oneshot::Receiver<SecretKey>,
	event_send: mpsc::Sender<Event>,
}

impl Socket {
	pub fn new<T: ToSocketAddrs>(
		addr: T,
		ssrc: u32,
	) -> std::io::Result<(Self, KeySender, Listener)> {
		debug!("New");
		let (key_send, key_recv) = oneshot::channel();
		let (event_send, event_recv) = mpsc::channel(16);
		let addr = addr
			.to_socket_addrs()?
			.next()
			.ok_or_else(|| std::io::ErrorKind::UnexpectedEof)?;

		let socket = Self {
			addr,
			ssrc,
			key_recv,
			event_send,
		};
		Ok((socket, key_send, event_recv))
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

		let writer = write(
			Arc::clone(&sock),
			self.ssrc,
			self.addr,
			self.key_recv,
			self.event_send.clone(),
		);
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
	key_recv: oneshot::Receiver<SecretKey>,
	mut event_send: mpsc::Sender<Event>,
) -> Result<(), Box<dyn Error>> {
	let mut sequence = 0u16;
	let mut timestamp = 0u32;
	let mut buf = Vec::<u8>::with_capacity(512);
	let mut nonce = [0u8; 24];
	let mut packet = [0u8; 2048];
	packet[0] = 0x80;
	packet[1] = 0x78;
	(&mut packet[8..12]).write_u32::<BigEndian>(ssrc)?;

	let key = key_recv.await?;
	let (frame_send, mut frame_recv) = mpsc::channel(16);
	event_send.send(Event::VoiceFrameSender(frame_send)).await?;
	let cipher = XSalsa20Poly1305::new_from_slice(&key).map_err(|_| "Invalid encryption key")?;

	while let Some(frame) = frame_recv.next().await {
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
		cipher
			.encrypt_in_place(n, &[], &mut buf)
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
	Ok(())
}
