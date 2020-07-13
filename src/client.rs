use crate::{api, protocol, types};
use reqwest::multipart::{Form, Part};
use reqwest::{header, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt;
use std::time::Duration;

const URL_PREFIX: &str = "https://discord.com/api/v6/";

#[derive(Debug)]
pub enum Error {
	Builder,
	Redirect,
	TimedOut,
	BadRequest,
	InvalidToken,
	NotPermitted,
	NotFound,
	RateLimited,
	GatewayUnavailable,
	Response(u16),
	Decode(serde_json::Error),
	Other(reqwest::Error),
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Error::Builder => write!(f, "Builder error"),
			Error::Redirect => write!(f, "Reached redirect limit"),
			Error::TimedOut => write!(f, "Request timed out"),
			Error::BadRequest => write!(f, "Bad request"),
			Error::InvalidToken => write!(f, "Invalid token"),
			Error::NotPermitted => write!(f, "Not permitted"),
			Error::NotFound => write!(f, "Not found"),
			Error::RateLimited => write!(f, "Rate limited"),
			Error::GatewayUnavailable => write!(f, "Gateway unavailable"),
			Error::Response(c) => write!(f, "Response code {}", *c),
			Error::Decode(e) => fmt::Display::fmt(e, f),
			Error::Other(e) => fmt::Display::fmt(e, f),
		}
	}
}

impl From<reqwest::Error> for Error {
	fn from(e: reqwest::Error) -> Self {
		use Error::*;

		if e.is_builder() {
			Builder
		} else if e.is_redirect() {
			Redirect
		} else if e.is_timeout() {
			TimedOut
		} else if let Some(status) = e.status() {
			match status.as_u16() {
				400 => BadRequest,
				401 => InvalidToken,
				403 => NotPermitted,
				404 => NotFound,
				429 => RateLimited,
				502 => GatewayUnavailable,
				c => Response(c),
			}
		} else {
			Other(e)
		}
	}
}

impl From<header::InvalidHeaderValue> for Error {
	fn from(_: header::InvalidHeaderValue) -> Self {
		Error::Builder
	}
}

#[derive(Clone, Debug)]
pub struct Client {
	client: reqwest::Client,
}

impl Client {
	pub fn new(token: String) -> Result<Self, Error> {
		let mut headers = header::HeaderMap::new();
		headers.insert(
			header::USER_AGENT,
			header::HeaderValue::from_str(&format!(
				"discord-async-rs (github.com/kebabtent/discord-async-rs, v{})",
				env!("CARGO_PKG_VERSION")
			))?,
		);
		headers.insert(
			header::AUTHORIZATION,
			header::HeaderValue::from_str(&format!("Bot {}", token))?,
		);

		let client = reqwest::ClientBuilder::new()
			.default_headers(headers)
			.timeout(Duration::from_secs(10))
			.use_rustls_tls()
			.build()?;

		Ok(Self { client })
	}

	async fn post<S, D>(&self, url: &str, body: S) -> Result<D, Error>
	where
		S: Serialize,
		D: DeserializeOwned,
	{
		decode(
			self.client
				.post(&format!("{}{}", URL_PREFIX, url))
				.header(header::CONTENT_TYPE, "application/json")
				.json(&body)
				.send()
				.await?,
		)
		.await
	}

	async fn post_multipart<D>(&self, url: &str, multipart: Form) -> Result<D, Error>
	where
		D: DeserializeOwned,
	{
		decode(
			self.client
				.post(&format!("{}{}", URL_PREFIX, url))
				.multipart(multipart)
				.send()
				.await?,
		)
		.await
	}

	async fn delete(&self, url: &str) -> Result<(), Error> {
		self.client
			.delete(&format!("{}{}", URL_PREFIX, url))
			.send()
			.await?;
		Ok(())
	}

	pub async fn create_message<T: Into<types::ChannelId>, U: Into<api::CreateMessage>>(
		&self,
		channel_id: T,
		content: U,
	) -> Result<protocol::Message, Error> {
		let content = content.into();
		let url = format!("channels/{}/messages", channel_id.into());
		if let Some(attachment) = content.file {
			let mut multipart = Form::new().part(
				"file",
				Part::bytes(attachment.data).file_name(attachment.name),
			);
			if let Some(text) = content.content {
				multipart = multipart.text("content", text);
			}
			self.post_multipart(&url, multipart).await
		} else {
			self.post(&url, content).await
		}
	}

	pub async fn delete_message<T: Into<(types::ChannelId, types::MessageId)>>(
		&self,
		ids: T,
	) -> Result<(), Error> {
		let (channel_id, message_id) = ids.into();
		self.delete(&format!("channels/{}/messages/{}", channel_id, message_id))
			.await
	}
}

#[inline]
async fn decode<D: DeserializeOwned>(res: Response) -> Result<D, Error> {
	let full = res.bytes().await?;
	serde_json::from_slice(&full).map_err(|e| Error::Decode(e))
}
