use super::{EncodeError, FRAME_LENGTH_MS, SAMPLE_RATE};
use bytes::{Bytes, BytesMut};
use futures::Stream;
use tokio_util::codec::Decoder;

pub struct PcmFrame(Bytes);

impl PcmFrame {
	pub fn new(inner: Bytes) -> Self {
		Self(inner)
	}

	pub fn len(&self) -> usize {
		self.0.len()
	}
}

impl AsRef<[u8]> for PcmFrame {
	fn as_ref(&self) -> &[u8] {
		&*self.0
	}
}

pub trait PcmStream:
	Stream<Item = Result<PcmFrame, EncodeError>> + Unpin + Send + Sync + 'static
{
	fn is_stereo(&self) -> bool;
	fn frame_sample_size(&self) -> usize {
		frame_sample_size(self.is_stereo())
	}
}

pub struct PcmCodec {
	frame_sample_size: usize,
}

impl PcmCodec {
	pub fn new(frame_sample_size: usize) -> Self {
		Self { frame_sample_size }
	}
}

impl Decoder for PcmCodec {
	type Item = PcmFrame;
	type Error = std::io::Error;

	fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
		// Each sample is 2 bytes (i16)
		let len = 2 * self.frame_sample_size;
		src.reserve(len);
		if src.len() >= len {
			Ok(Some(PcmFrame::new(src.split_to(len).freeze())))
		} else {
			Ok(None)
		}
	}

	fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
		match self.decode(src)? {
			Some(frame) => Ok(Some(frame)),
			None => Ok(None),
		}
	}
}

pub fn frame_sample_size(stereo: bool) -> usize {
	let size = SAMPLE_RATE * FRAME_LENGTH_MS / 1_000;
	if stereo {
		2 * size
	} else {
		size
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::voice::source::PcmFile;
	use tokio_stream::StreamExt;

	#[tokio::test]
	async fn stream() {
		let mut source = PcmFile::new("../ma.pcm", true).await.unwrap();
		assert_eq!(source.next().await.unwrap().unwrap().len(), 2 * 1920);
	}
}
