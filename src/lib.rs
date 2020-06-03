use self::gateway::{Command, Event};
use chrono::prelude::{DateTime, Utc};
use chrono::TimeZone;
use futures::channel::mpsc;
use futures::stream::{self, SplitSink, SplitStream};
use futures::{pin_mut, select};
use futures::{FutureExt, SinkExt, StreamExt};
pub use gateway::{Connector, Error, Gateway};
use log::{debug, info, warn};
use never::Never;
use serde::de::{Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::{task, time};

// mod api;
mod gateway;
mod protocol;

// TODO: backpressure?
pub type GuildStream = mpsc::UnboundedReceiver<Guild>;
type GuildSink = mpsc::UnboundedSender<Guild>;

pub type GuildEventStream = mpsc::UnboundedReceiver<GuildEvent>;
type GuildEventSink = mpsc::UnboundedSender<GuildEvent>;

type CommandStream = mpsc::UnboundedReceiver<Command>;
type CommandSink = mpsc::UnboundedSender<Command>;

type Guilds = HashMap<Snowflake, (bool, GuildEventSink)>;

const DISCORD_EPOCH: u64 = 1_420_070_400_000;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
pub struct Snowflake(u64);

impl Snowflake {
	pub fn date_time(&self) -> DateTime<Utc> {
		let timestamp = (self.0 >> 22) + DISCORD_EPOCH;
		Utc.timestamp_millis(timestamp as i64)
	}

	pub fn worker(&self) -> u8 {
		((self.0 & 0x3E0000) >> 17) as u8
	}

	pub fn process(&self) -> u8 {
		((self.0 & 0x1F000) >> 12) as u8
	}

	pub fn increment(&self) -> u16 {
		(self.0 & 0xFFF) as u16
	}
}

impl fmt::Display for Snowflake {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.0)
	}
}

impl From<u64> for Snowflake {
	fn from(snowflake: u64) -> Self {
		Self(snowflake)
	}
}

impl<'de> Deserialize<'de> for Snowflake {
	fn deserialize<D>(deserializer: D) -> Result<Snowflake, D::Error>
	where
		D: Deserializer<'de>,
	{
		struct SnowflakeVisitor;

		impl<'de> Visitor<'de> for SnowflakeVisitor {
			type Value = Snowflake;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("u64 or string storing a snowflake id")
			}

			fn visit_u64<E>(self, val: u64) -> Result<Snowflake, E>
			where
				E: serde::de::Error,
			{
				Ok(Snowflake(val))
			}

			fn visit_str<E>(self, val: &str) -> Result<Snowflake, E>
			where
				E: serde::de::Error,
			{
				val.parse()
					.map(|v| Snowflake(v))
					.map_err(|_| E::invalid_value(Unexpected::Str(val), &self))
			}
		}

		deserializer.deserialize_any(SnowflakeVisitor)
	}
}

pub struct Discord {
	token: String,
}

impl Discord {
	pub fn new(token: String) -> Self {
		Self { token }
	}

	pub fn start(self) -> (GuildStream, CommandSink) {
		let (guild_send, guild_recv) = mpsc::unbounded();
		let (command_send, command_recv) = mpsc::unbounded();
		task::spawn(start_discord(self.token, guild_send, command_recv));
		(guild_recv, command_send)
	}
}

pub struct Guild {
	pub id: Snowflake,
	pub name: String,
	pub available: bool,
	pub recv: GuildEventStream,
}

pub enum GuildEvent {
	Offline,
	Online,
	Event(Event),
}

async fn start_discord(token: String, mut guild_send: GuildSink, mut command_recv: CommandStream) {
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
			Ok(_) => {
				info!("Connection shutdown successfully");
				return;
			}
			Err(e) => warn!("Connection error: {:?}", e),
		}
		for (available, event_send) in guilds.values_mut() {
			*available = false;
			let _ = event_send.unbounded_send(GuildEvent::Offline);
		}
		time::delay_for(Duration::from_secs(3)).await; // TODO: exponential backoff?
	}
}

struct Request {}

async fn connect(
	token: &str,
	session_id: &mut Option<String>,
	sequence: Arc<AtomicU64>,
	guilds: &mut Guilds,
	guild_send: &mut GuildSink,
	command_recv: &mut CommandStream,
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
	let read_fut = read(reader, guild_send, guilds, sequence).fuse();
	pin_mut!(read_fut);

	select! {
		res = write_fut => res,
		res = read_fut => res,
	}
}

async fn write(
	mut gateway: SplitSink<Gateway, Command>,
	command_recv: &mut CommandStream,
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
	/*loop {
		let command = match select! {
			c = command_recv.next() => c,
			hb = heartbeat.next() => hb,
		} {
			Some(c) => c,
			None => break,
		};
		gateway.send(command).await?;
	}*/

	debug!("Writer shutdown");

	Err(Error::Close(None))
}

async fn read(
	mut gateway: SplitStream<Gateway>,
	guild_send: &mut GuildSink,
	guilds: &mut Guilds,
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

		match event {
			Event::GuildCreate(gc) => {
				if !guilds.contains_key(&gc.id) {
					// 'New' guild (not yet known)
					let (send, recv) = mpsc::unbounded();
					guilds.insert(gc.id, (!gc.unavailable, send));
					let guild = Guild {
						id: gc.id,
						name: gc.name,
						available: false,
						recv,
					};
					let _ = guild_send.unbounded_send(guild);
				}

				let (available, send) = guilds.get_mut(&gc.id).unwrap();
				if *available == gc.unavailable {
					// Availability toggled, notify guild
					*available = !gc.unavailable;
					let event = if gc.unavailable {
						GuildEvent::Offline
					} else {
						GuildEvent::Online
					};
					let _ = send.unbounded_send(event);
				}
			}
			Event::HearbeatAck => {}
			Event::Unknown(_) => {}
			Event::Hello(_) | Event::Ready(_) | Event::Resumed => {}
			_ => {}
		}
	}

	debug!("Reader shutdown");

	Err(Error::Close(None))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn snowflake() {
		let id = Snowflake(175_928_847_299_117_063);
		let time = Utc.ymd(2016, 04, 30).and_hms_milli(11, 18, 25, 796);
		assert_eq!(id.date_time(), time);
		assert_eq!(id.worker(), 1);
		assert_eq!(id.process(), 0);
		assert_eq!(id.increment(), 7);
	}
}
