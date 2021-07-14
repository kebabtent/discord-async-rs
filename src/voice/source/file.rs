use crate::voice::pcm::{frame_sample_size, PcmCodec, PcmFrame, PcmStream};
use crate::voice::{EncodeError, OpusStream};
use bytes::BytesMut;
use futures::{Stream, StreamExt};
use pin_project::pin_project;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::fs::File;
use tokio_util::codec::{Decoder, FramedRead};

#[pin_project]
pub struct PcmFile {
	stereo: bool,
	#[pin]
	source: FramedRead<File, PcmCodec>,
}

impl PcmFile {
	async fn new<P: AsRef<Path>>(path: P, stereo: bool) -> Result<Self, std::io::Error> {
		let source = FramedRead::new(
			File::open(path).await?,
			PcmCodec::new(frame_sample_size(stereo)),
		);
		Ok(Self { stereo, source })
	}
}

impl PcmStream for PcmFile {
	fn is_stereo(&self) -> bool {
		self.stereo
	}
}

impl Stream for PcmFile {
	type Item = Result<PcmFrame, EncodeError>;
	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		self.project().source.poll_next(cx).map_err(|e| e.into())
	}
}

pub async fn pcm_file<P: AsRef<Path>>(
	path: P,
	stereo: bool,
	bitrate: u32,
) -> Result<OpusStream, EncodeError> {
	let file = PcmFile::new(path, stereo).await?;
	Ok(OpusStream::new(file, bitrate)?)
}
