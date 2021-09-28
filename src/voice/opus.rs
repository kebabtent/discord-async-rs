use super::pcm::PcmStream;
use super::{EncodeError, FRAME_LENGTH_MS, SAMPLE_RATE};
use byteorder::{LittleEndian, ReadBytesExt};
use bytes::{Bytes, BytesMut};
use futures::stream::FusedStream;
use futures::{ready, Future, Stream};
use pin_project::pin_project;
use std::fmt;
use std::io::Cursor;
use std::ops::Deref;
use std::pin::Pin;
use std::task::Poll::*;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::{sleep_until, Instant, Sleep};

const SILENCE_FRAME_COUNT: u8 = 5;
const SILENCE_FRAME: [u8; 3] = [0xF8, 0xFF, 0xFE];

pub struct OpusFrame(Bytes);

impl OpusFrame {
	pub fn len(&self) -> usize {
		self.0.len()
	}
}

impl Deref for OpusFrame {
	type Target = [u8];

	fn deref(&self) -> &[u8] {
		&self.0[..]
	}
}

impl AsRef<[u8]> for OpusFrame {
	fn as_ref(&self) -> &[u8] {
		&self.0[..]
	}
}

impl fmt::Debug for OpusFrame {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("OpusFrame").finish()
	}
}

#[pin_project]
struct OpusEncoder {
	#[pin]
	source: Box<dyn PcmStream>,
	encoder: opus::Encoder,
	buf: Vec<i16>,
	frame_size: usize,
	output: BytesMut,
}

impl OpusEncoder {
	fn new<S: PcmStream>(source: S, bitrate: u32) -> Result<Self, EncodeError> {
		let buf = vec![0i16; source.frame_sample_size()];
		let channels = if source.is_stereo() {
			opus::Channels::Stereo
		} else {
			opus::Channels::Mono
		};
		let source = Box::new(source);

		let bitrate = bitrate.clamp(6_000, 510_000);
		let frame_size = (bitrate as usize) * FRAME_LENGTH_MS / 8 / 1000;

		let output = BytesMut::with_capacity(frame_size);
		let mut encoder =
			opus::Encoder::new(SAMPLE_RATE as u32, channels, opus::Application::Audio)?;
		encoder.set_vbr(false)?;
		encoder.set_bitrate(opus::Bitrate::Bits(bitrate as i32))?;

		Ok(Self {
			source,
			encoder,
			buf,
			frame_size,
			output,
		})
	}
}

impl Stream for OpusEncoder {
	type Item = Result<Bytes, EncodeError>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let mut this = self.project();
		let mut source = match ready!(this.source.poll_next(cx)) {
			Some(f) => {
				let f = f?;
				debug_assert_eq!(f.len(), 2 * this.buf.len(), "pcm frame size");
				Cursor::new(f)
			}
			None => return Ready(None),
		};
		source.read_i16_into::<LittleEndian>(&mut this.buf)?;
		let frame_size = *this.frame_size;
		unsafe {
			// SAFETY: we try to write `frame_size` bytes and if we don't manage to,
			// we set the length back to 0.
			this.output.reserve(frame_size);
			this.output.set_len(frame_size);
			let mut frame = this.output.split();
			match this.encoder.encode(&this.buf, &mut frame) {
				Err(e) => {
					frame.set_len(0);
					Ready(Some(Err(e.into())))
				}
				Ok(s) if s != frame_size => {
					frame.set_len(0);
					Ready(Some(Err(EncodeError::FrameSize)))
				}
				Ok(_) => Ready(Some(Ok(frame.freeze()))),
			}
		}
	}
}

#[pin_project]
pub struct OpusStream {
	delay: Pin<Box<Sleep>>,
	deadline: Instant,
	duration: Duration,
	has_delayed: bool,
	#[pin]
	encoder: OpusEncoder,
	silence_frames: u8,
	done: bool,
}

impl OpusStream {
	pub fn new<S: PcmStream>(source: S, bitrate: u32) -> Result<Self, EncodeError> {
		let encoder = OpusEncoder::new(source, bitrate)?;
		let duration = Duration::from_millis(FRAME_LENGTH_MS as u64);
		let deadline = Instant::now();
		let stream = Self {
			delay: Box::pin(sleep_until(deadline)),
			deadline,
			duration,
			has_delayed: false,
			encoder,
			silence_frames: SILENCE_FRAME_COUNT,
			done: false,
		};
		assert_unpin(&stream);
		Ok(stream)
	}
}

impl Stream for OpusStream {
	type Item = Result<OpusFrame, EncodeError>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let this = self.project();
		if *this.done {
			return Ready(None);
		}

		if !*this.has_delayed {
			ready!(this.delay.as_mut().poll(cx));
			*this.has_delayed = true;
		}

		let val = if *this.silence_frames > 0 {
			*this.silence_frames -= 1;
			Ok(Bytes::from_static(&SILENCE_FRAME))
		} else {
			let val = ready!(this.encoder.poll_next(cx));
			match val {
				Some(v) => v,
				None => {
					*this.done = true;
					return Ready(None);
				}
			}
		};
		*this.has_delayed = false;
		let now = Instant::now();
		let passed = now - *this.deadline;
		let micros = this.duration.as_micros().saturating_sub(passed.as_micros()) as u64;
		let deadline = now + Duration::from_micros(micros);
		this.delay.as_mut().reset(deadline);
		*this.deadline = deadline;

		Ready(Some(val.map(|v| OpusFrame(v))))
	}
}

impl FusedStream for OpusStream {
	fn is_terminated(&self) -> bool {
		self.done
	}
}

impl fmt::Debug for OpusStream {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("OpusStream").finish()
	}
}

fn assert_unpin<T: Unpin>(_t: &T) {}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::voice::source::PcmFile;
	use byteorder::{LittleEndian, ReadBytesExt};
	use futures::StreamExt;
	use std::fs::File;
	use std::io::Read;

	/*#[test]
	fn opus() {
		let mut source = File::open("../ma.pcm").unwrap();
		let mut buf = [0i16; 2 * 960];
		source.read_i16_into::<LittleEndian>(&mut buf).unwrap();
		let mut packet = [0u8; 1024];
		let mut enc = opus::Encoder::new(
			SAMPLE_RATE as u32,
			opus::Channels::Stereo,
			opus::Application::Audio,
		)
		.unwrap();
		enc.set_vbr(false).unwrap();
		let bitrate = 384_000;
		enc.set_bitrate(opus::Bitrate::Bits(bitrate)).unwrap();
		println!("bitrate = {:?}", enc.get_bitrate().unwrap());
		println!("VBR = {:?}", enc.get_vbr().unwrap());
		println!("SR = {:?}", enc.get_sample_rate().unwrap());
		let size = enc.encode(&buf, &mut packet).unwrap();
		println!("SIZE = {}", size);
		assert_eq!(size * 400, bitrate as usize); // 8(bit->byte)/20(frame length ms)*1000(ms->s)
	}*/

	/*#[tokio::test]
	async fn encoder() {
		let bitrate = 96_000;
		let source = PcmFile::new("../ma.pcm", true).await.unwrap();
		let mut enc = OpusEncoder::new(source, bitrate).unwrap();
		let frame = enc.next().await.unwrap().unwrap();
		assert_eq!(frame.len(), bitrate as usize / 400);
	}*/
}
