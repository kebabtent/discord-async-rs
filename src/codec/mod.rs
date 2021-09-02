pub use self::json::JsonCodec;
use crate::GatewayError;
use async_tungstenite::tokio::connect_async;
use async_tungstenite::tokio::ConnectStream;
use async_tungstenite::tungstenite;
use async_tungstenite::WebSocketStream;
use futures::{Sink, Stream};
use pin_project::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll};
use tungstenite::client::IntoClientRequest;
use tungstenite::handshake::client::Response;
use tungstenite::Message;

mod json;

pub trait Codec<S, D>: Sync + Send {
	fn encode(&mut self, command: S) -> Result<Message, GatewayError>;
	fn decode(&mut self, message: Message) -> Result<D, GatewayError>;
}

#[pin_project]
pub struct Connection<S, D> {
	#[pin]
	conn: WebSocketStream<ConnectStream>,
	codec: Box<dyn Codec<S, D>>,
}

impl<S, D> Connection<S, D> {
	pub async fn connect<R>(
		request: R,
		codec: Box<dyn Codec<S, D>>,
	) -> Result<(Self, Response), GatewayError>
	where
		R: IntoClientRequest + Unpin,
	{
		let (conn, res) = connect_async(request).await?;
		let conn = Self { conn, codec };
		Ok((conn, res))
	}
}

impl<S, D> Stream for Connection<S, D> {
	type Item = Result<D, GatewayError>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let project = self.project();
		match project.conn.poll_next(cx) {
			Poll::Ready(Some(Ok(message))) => Poll::Ready(Some(project.codec.decode(message))),
			Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e.into()))),
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}
}

impl<S, D> Sink<S> for Connection<S, D> {
	type Error = GatewayError;

	fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.project().conn.poll_ready(cx).map_err(|e| e.into())
	}

	fn start_send(self: Pin<&mut Self>, item: S) -> Result<(), Self::Error> {
		let project = self.project();
		let message = project.codec.encode(item)?;
		project.conn.start_send(message).map_err(|e| e.into())
	}

	fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.project().conn.poll_flush(cx).map_err(|e| e.into())
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		self.project().conn.poll_close(cx).map_err(|e| e.into())
	}
}
