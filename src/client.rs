use crate::ApiError;
use discord_types::command::{RequestGuildMembers, UpdateVoiceState};
use discord_types::request;
use discord_types::{
	AllowedMentions, ApplicationCommand, ApplicationCommandOption, ApplicationId, ChannelId,
	Command, Embed, GuildId, InteractionResponseType, Member, Message, MessageId, RoleId,
	Snowflake, UserId,
};
use futures::channel::mpsc;
use reqwest::multipart::{Form, Part};
use reqwest::{header, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

const URL_PREFIX: &str = "https://discord.com/api/v8/";

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
	Api(ApiError),
	Decode(serde_json::Error),
	Other(reqwest::Error),
}

impl Error {
	pub fn is_not_found(&self) -> bool {
		match self {
			Error::NotFound => true,
			_ => false,
		}
	}
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
			Error::Api(e) => fmt::Display::fmt(e, f),
			Error::Decode(e) => fmt::Display::fmt(e, f),
			Error::Other(e) => fmt::Display::fmt(e, f),
		}
	}
}

impl std::error::Error for Error {}

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
		Self::Builder
	}
}

impl From<ApiError> for Error {
	fn from(e: ApiError) -> Self {
		Self::Api(e)
	}
}

impl From<serde_json::Error> for Error {
	fn from(e: serde_json::Error) -> Self {
		Self::Decode(e)
	}
}

pub trait OptionalResult {
	type Inner;
	type Error;
	fn optional(self) -> Result<Option<Self::Inner>, Self::Error>;
}

impl<T> OptionalResult for Result<T, Error> {
	type Inner = T;
	type Error = Error;
	fn optional(self) -> Result<Option<T>, Error> {
		match self {
			Ok(v) => Ok(Some(v)),
			Err(e) if e.is_not_found() => Ok(None),
			Err(e) => Err(e),
		}
	}
}

#[derive(Clone, Debug)]
pub struct Client {
	client: reqwest::Client,
	command_send: mpsc::Sender<Command>,
}

impl Client {
	pub fn new(token: String, command_send: mpsc::Sender<Command>) -> Result<Self, Error> {
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

		Ok(Self {
			client,
			command_send,
		})
	}

	pub fn request_guild_members(&mut self, guild_id: GuildId) -> Result<(), ()> {
		let req = RequestGuildMembers {
			guild_id,
			query: String::new(),
			limit: 0,
			presences: None,
			user_ids: HashSet::new(),
			nonce: None,
		};
		self.command_send.try_send(req.into()).map_err(|_| ())
	}

	pub fn update_voice_state(
		&mut self,
		guild_id: GuildId,
		channel_id: Option<ChannelId>,
		self_mute: bool,
		self_deaf: bool,
	) -> Result<(), ()> {
		let upd = UpdateVoiceState {
			guild_id,
			channel_id,
			self_mute,
			self_deaf,
		};
		self.command_send.try_send(upd.into()).map_err(|_| ())
	}

	async fn get<D>(&self, url: &str) -> Result<D, Error>
	where
		D: DeserializeOwned,
	{
		decode(
			self.client
				.get(&format!("{}{}", URL_PREFIX, url))
				.send()
				.await?,
		)
		.await
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

	async fn post_discard<S>(&self, url: &str, body: S) -> Result<(), Error>
	where
		S: Serialize,
	{
		check_response_code(
			self.client
				.post(&format!("{}{}", URL_PREFIX, url))
				.header(header::CONTENT_TYPE, "application/json")
				.json(&body)
				.send()
				.await?,
		)
		.await?;
		Ok(())
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

	async fn put<S>(&self, url: &str, body: S) -> Result<(), Error>
	where
		S: Serialize,
	{
		check_response_code(
			self.client
				.put(&format!("{}{}", URL_PREFIX, url))
				.header(header::CONTENT_TYPE, "application/json")
				.json(&body)
				.send()
				.await?,
		)
		.await?;
		Ok(())
	}

	async fn delete(&self, url: &str) -> Result<(), Error> {
		self.client
			.delete(&format!("{}{}", URL_PREFIX, url))
			.send()
			.await?;
		Ok(())
	}

	pub fn create_message<T: Into<ChannelId>>(&self, channel_id: T) -> CreateMessageBuilder<'_> {
		CreateMessageBuilder {
			client: &self,
			channel_id: channel_id.into(),
		}
	}

	pub async fn delete_message<T: Into<(ChannelId, MessageId)>>(
		&self,
		ids: T,
	) -> Result<(), Error> {
		let (channel_id, message_id) = ids.into();
		self.delete(&format!("channels/{}/messages/{}", channel_id, message_id))
			.await
	}

	pub fn create_guild_ban<T: Into<GuildId>, U: Into<UserId>>(
		&self,
		guild_id: T,
		user_id: U,
	) -> CreateGuildBan<'_> {
		CreateGuildBan {
			client: &self,
			guild_id: guild_id.into(),
			user_id: user_id.into(),
			cgb: request::CreateGuildBan {
				delete_message_days: None,
				reason: None,
			},
		}
	}

	pub(crate) async fn commands(
		&self,
		application_id: ApplicationId,
		guild_id: GuildId,
	) -> Result<Vec<ApplicationCommand>, Error> {
		self.get(&format!(
			"applications/{}/guilds/{}/commands",
			application_id, guild_id
		))
		.await
	}

	pub async fn create_command(
		&self,
		application_id: ApplicationId,
		guild_id: GuildId,
		name: &str,
		description: &str,
		options: Vec<ApplicationCommandOption>,
	) -> Result<serde_json::Value, Error> {
		let body = request::CreateCommand {
			name,
			description,
			options,
		};

		self.post(
			&format!(
				"applications/{}/guilds/{}/commands",
				application_id, guild_id
			),
			body,
		)
		.await
	}

	pub fn interaction_response<'a>(
		&'a self,
		interaction_id: Snowflake,
		token: &'a str,
	) -> InteractionResponse<'a> {
		InteractionResponse::new(self, interaction_id, token)
	}

	pub async fn get_guild_member(
		&self,
		guild_id: GuildId,
		user_id: UserId,
	) -> Result<Member, Error> {
		self.get(&format!("guilds/{}/members/{}", guild_id, user_id))
			.await
	}

	pub async fn add_guild_member_role(
		&self,
		guild_id: GuildId,
		user_id: UserId,
		role_id: RoleId,
	) -> Result<(), Error> {
		self.put(
			&format!("guilds/{}/members/{}/roles/{}", guild_id, user_id, role_id),
			(),
		)
		.await
	}

	pub async fn remove_guild_member_role(
		&self,
		guild_id: GuildId,
		user_id: UserId,
		role_id: RoleId,
	) -> Result<(), Error> {
		self.delete(&format!(
			"guilds/{}/members/{}/roles/{}",
			guild_id, user_id, role_id
		))
		.await
	}
}

async fn decode<D: DeserializeOwned>(res: Response) -> Result<D, Error> {
	let res = check_response_code(res).await?;
	let full = res.bytes().await?;
	serde_json::from_slice(&full).map_err(|e| Error::Decode(e))
}

async fn check_response_code(res: Response) -> Result<Response, Error> {
	match res.error_for_status_ref() {
		Ok(_) => Ok(res),
		Err(err) => {
			let full = match res.bytes().await {
				Ok(f) => f,
				Err(_) => return Err(err.into()),
			};
			let api_err = serde_json::from_slice::<ApiError>(&full);
			match api_err {
				Ok(err) => Err(err.into()),
				Err(_) => Err(err.into()),
			}
		}
	}
}

pub struct CreateMessageBuilder<'a> {
	client: &'a Client,
	channel_id: ChannelId,
}

impl<'a> CreateMessageBuilder<'a> {
	pub fn content(self, content: String) -> CreateMessage<'a> {
		CreateMessage {
			client: self.client,
			channel_id: self.channel_id,
			attachment: None,
			cm: request::CreateMessage {
				content: Some(content),
				embed: None,
			},
		}
	}

	pub fn embed(self, embed: Embed) -> CreateMessage<'a> {
		CreateMessage {
			client: self.client,
			channel_id: self.channel_id,
			attachment: None,
			cm: request::CreateMessage {
				content: None,
				embed: Some(embed),
			},
		}
	}
}

pub struct CreateMessage<'a> {
	client: &'a Client,
	channel_id: ChannelId,
	attachment: Option<request::Attachment>,
	cm: request::CreateMessage,
}

impl<'a> CreateMessage<'a> {
	pub fn content(mut self, content: String) -> Self {
		self.cm.content = Some(content);
		self
	}

	pub fn embed(mut self, embed: Embed) -> Self {
		self.cm.embed = Some(embed);
		self
	}

	pub fn attachment(mut self, attachment: request::Attachment) -> Self {
		self.attachment = Some(attachment);
		self
	}

	pub async fn send(self) -> Result<Message, Error> {
		let url = format!("channels/{}/messages", self.channel_id);

		if let Some(attachment) = self.attachment {
			let multipart = Form::new()
				.part(
					"file",
					Part::bytes(attachment.data).file_name(attachment.name),
				)
				.text("payload_json", serde_json::to_string(&self.cm)?);
			self.client.post_multipart(&url, multipart).await
		} else {
			self.client.post(&url, self.cm).await
		}
	}
}

pub struct CreateGuildBan<'a> {
	client: &'a Client,
	guild_id: GuildId,
	user_id: UserId,
	cgb: request::CreateGuildBan<'a>,
}

impl<'a> CreateGuildBan<'a> {
	pub fn delete_message_days(mut self, delete_message_days: u8) -> Self {
		self.cgb.delete_message_days = Some(delete_message_days);
		self
	}

	pub fn reason(mut self, reason: &'a str) -> Self {
		self.cgb.reason = Some(reason);
		self
	}

	pub async fn send(self) -> Result<(), Error> {
		self.client
			.put(
				&format!("guilds/{}/bans/{}", self.guild_id, self.user_id),
				self.cgb,
			)
			.await
	}
}

pub struct InteractionResponse<'a> {
	client: &'a Client,
	interaction_id: Snowflake,
	token: &'a str,
	ir: request::InteractionResponse<'a>,
}

impl<'a> InteractionResponse<'a> {
	fn new(client: &'a Client, interaction_id: Snowflake, token: &'a str) -> Self {
		Self {
			client,
			interaction_id,
			token,
			ir: request::InteractionResponse {
				response_type: InteractionResponseType::ChannelMessage,
				data: None,
			},
		}
	}

	fn data(&mut self) -> &mut request::InteractionCallbackData<'a> {
		if self.ir.data.is_none() {
			self.ir.data = Some(Default::default());
		}
		self.ir.data.as_mut().unwrap()
	}

	pub fn content(mut self, content: &'a str) -> Self {
		self.data().content = Some(content);
		self
	}

	pub fn embed(mut self, embed: Embed) -> Self {
		self.data().embeds.push(embed);
		self
	}

	pub fn ephemeral(mut self) -> Self {
		self.data().flags = Some(64);
		self
	}

	pub fn deferred(mut self) -> Self {
		self.ir.response_type = InteractionResponseType::DeferredChannelMessage;
		self
	}

	pub fn allowed_mentions(mut self, m: AllowedMentions) -> Self {
		self.data().allowed_mentions = Some(m);
		self
	}

	pub async fn send(mut self) -> Result<(), Error> {
		if let Some(data) = &self.ir.data {
			if data.content.is_none() && data.embeds.is_empty() {
				self.ir.data = None;
			}
		}

		self.client
			.post_discard(
				&format!(
					"interactions/{}/{}/callback",
					self.interaction_id, self.token
				),
				self.ir,
			)
			.await
	}
}
