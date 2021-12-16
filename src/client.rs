use crate::ApiError;
use discord_types::command::{RequestGuildMembers, UpdateVoiceState};
use discord_types::request;
use discord_types::{
	AllowedMentions, ApplicationCommand, ApplicationCommandOption, ApplicationId, ButtonStyle,
	ChannelId, Command, Component, ComponentType, Embed, GuildId, InteractionId,
	InteractionResponseType, Member, Message, MessageId, PartialEmoji, RoleId, User, UserId,
};
use futures::channel::mpsc;
use reqwest::multipart::{Form, Part};
use reqwest::{header, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

type CowString = std::borrow::Cow<'static, str>;

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
	command_send: Option<mpsc::Sender<Command>>,
}

impl Client {
	pub fn new(token: &str, command_send: Option<mpsc::Sender<Command>>) -> Result<Self, Error> {
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

	fn command_send(&mut self) -> Result<&mut mpsc::Sender<Command>, ()> {
		self.command_send.as_mut().ok_or(())
	}

	pub fn request_guild_members(&mut self, guild_id: GuildId) -> Result<(), ()> {
		let req = RequestGuildMembers {
			guild_id,
			query: "".into(),
			limit: 0,
			presences: None,
			user_ids: HashSet::new(),
			nonce: None,
		};

		self.command_send()?.try_send(req.into()).map_err(|_| ())
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
		self.command_send()?.try_send(upd.into()).map_err(|_| ())
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

	async fn patch<S, D>(&self, url: &str, body: S) -> Result<D, Error>
	where
		S: Serialize,
		D: DeserializeOwned,
	{
		decode(
			self.client
				.patch(&format!("{}{}", URL_PREFIX, url))
				.header(header::CONTENT_TYPE, "application/json")
				.json(&body)
				.send()
				.await?,
		)
		.await
	}

	async fn delete(&self, url: &str) -> Result<(), Error> {
		check_response_code(
			self.client
				.delete(&format!("{}{}", URL_PREFIX, url))
				.send()
				.await?,
		)
		.await?;
		Ok(())
	}

	pub async fn get_message(
		&self,
		channel_id: ChannelId,
		message_id: MessageId,
	) -> Result<Message, Error> {
		self.get(&format!("channels/{}/messages/{}", channel_id, message_id))
			.await
	}

	pub async fn get_reactions(
		&self,
		channel_id: ChannelId,
		message_id: MessageId,
		emoji: &PartialEmoji,
		after: Option<UserId>,
		limit: Option<u8>,
	) -> Result<Vec<User>, Error> {
		let name = emoji.name.as_deref().ok_or(Error::BadRequest)?;
		let name = match emoji.id {
			Some(id) => format!("{}:{}", name, id),
			None => name.to_string(),
		};
		let name = urlencoding::encode(&name);
		let limit = limit.unwrap_or(100).min(100);
		let params = if let Some(user_id) = after {
			format!("&after={}", user_id)
		} else {
			String::new()
		};

		self.get(&format!(
			"channels/{}/messages/{}/reactions/{}?limit={}{}",
			channel_id, message_id, name, limit, params
		))
		.await
	}

	pub fn create_message(&self, channel_id: ChannelId) -> CreateMessageBuilder<'_> {
		CreateMessageBuilder {
			client: &self,
			channel_id: channel_id.into(),
		}
	}

	pub fn edit_message(&self, channel_id: ChannelId, message_id: MessageId) -> EditMessage<'_> {
		EditMessage::new(self, channel_id, message_id)
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
		interaction_id: InteractionId,
		token: &'a str,
		component: bool,
	) -> InteractionResponse<'a> {
		InteractionResponse::new(self, interaction_id, token, component)
	}

	pub async fn get_guild_members(
		&self,
		guild_id: GuildId,
		after: Option<UserId>,
		limit: Option<u16>,
	) -> Result<Vec<Member>, Error> {
		let limit = limit.unwrap_or(1000).min(1000);
		let param = match after {
			Some(a) => format!("&after={}", a),
			None => String::new(),
		};
		self.get(&format!(
			"guilds/{}/members?limit={}{}",
			guild_id, limit, param
		))
		.await
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
	fn into(self) -> CreateMessage<'a> {
		CreateMessage {
			client: self.client,
			channel_id: self.channel_id,
			attachment: None,
			cm: request::CreateMessage {
				content: None,
				embeds: Vec::new(),
				components: Vec::new(),
			},
		}
	}

	pub fn content<T: Into<CowString>>(self, content: T) -> CreateMessage<'a> {
		let mut cm = self.into();
		cm.cm.content = Some(content.into());
		cm
	}

	pub fn embed(self, embed: Embed) -> CreateMessage<'a> {
		let mut cm = self.into();
		cm.cm.embeds.push(embed);
		cm
	}

	pub fn embeds<I: IntoIterator<Item = Embed>>(self, embeds: I) -> CreateMessage<'a> {
		let mut cm = self.into();
		cm.cm.embeds = embeds.into_iter().collect();
		cm
	}
}

pub struct CreateMessage<'a> {
	client: &'a Client,
	channel_id: ChannelId,
	attachment: Option<request::Attachment>,
	cm: request::CreateMessage,
}

impl<'a> CreateMessage<'a> {
	pub fn content<T: Into<CowString>>(mut self, content: T) -> Self {
		self.cm.content = Some(content.into());
		self
	}

	pub fn embed(mut self, embed: Embed) -> Self {
		self.cm.embeds.push(embed);
		self
	}

	pub fn attachment(mut self, attachment: request::Attachment) -> Self {
		self.attachment = Some(attachment);
		self
	}

	pub fn component_rows<T: IntoIterator<Item = RowComponent>>(mut self, rows: T) -> Self {
		self.cm.components = rows.into_iter().map(|r| r.component).collect();
		self
	}

	pub fn component_row(mut self, row: RowComponent) -> Self {
		self.cm.components.push(row.component);
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

pub struct EditMessage<'a> {
	client: &'a Client,
	channel_id: ChannelId,
	message_id: MessageId,
	em: request::EditMessage,
}

impl<'a> EditMessage<'a> {
	fn new(client: &'a Client, channel_id: ChannelId, message_id: MessageId) -> Self {
		Self {
			client,
			channel_id,
			message_id,
			em: request::EditMessage {
				content: None,
				embeds: None,
				components: None,
			},
		}
	}

	pub fn content<T: Into<CowString>>(mut self, content: T) -> Self {
		self.em.content = Some(content.into());
		self
	}

	pub fn embed(mut self, embed: Embed) -> Self {
		self.em.embeds.get_or_insert_with(|| Vec::new()).push(embed);
		self
	}

	pub fn component_rows<T: IntoIterator<Item = RowComponent>>(mut self, rows: T) -> Self {
		self.em.components = Some(rows.into_iter().map(|r| r.component).collect());
		self
	}

	pub fn component_row(mut self, row: RowComponent) -> Self {
		self.em
			.components
			.get_or_insert_with(|| Vec::new())
			.push(row.component);
		self
	}

	pub async fn send(self) -> Result<Message, Error> {
		let url = format!("channels/{}/messages/{}", self.channel_id, self.message_id);
		self.client.patch(&url, self.em).await
	}
}

#[derive(Clone)]
pub struct RowComponent {
	component: Component,
}

impl RowComponent {
	pub fn new() -> Self {
		Self {
			component: Component {
				component_type: ComponentType::ActionRow,
				style: None,
				label: None,
				emoji: None,
				custom_id: None,
				url: None,
				disabled: None,
				default: None,
				components: Vec::with_capacity(5),
			},
		}
	}

	pub fn button(mut self, button: ButtonComponent) -> Self {
		self.component.components.push(button.component);
		self
	}
}

impl From<ButtonComponent> for RowComponent {
	fn from(button: ButtonComponent) -> RowComponent {
		RowComponent::new().button(button)
	}
}

#[derive(Clone)]
pub struct ButtonComponent {
	component: Component,
}

impl ButtonComponent {
	fn new(style: ButtonStyle) -> Self {
		Self {
			component: Component {
				component_type: ComponentType::Button,
				style: Some(style),
				label: None,
				emoji: None,
				custom_id: None,
				url: None,
				disabled: None,
				default: None,
				components: Vec::new(),
			},
		}
	}

	pub fn primary<T: Into<CowString>>(custom_id: T) -> Self {
		let mut button = Self::new(ButtonStyle::Primary);
		button.component.custom_id = Some(custom_id.into());
		button
	}

	pub fn secondary<T: Into<CowString>>(custom_id: T) -> Self {
		let mut button = Self::new(ButtonStyle::Secondary);
		button.component.custom_id = Some(custom_id.into());
		button
	}

	pub fn success<T: Into<CowString>>(custom_id: T) -> Self {
		let mut button = Self::new(ButtonStyle::Success);
		button.component.custom_id = Some(custom_id.into());
		button
	}

	pub fn danger<T: Into<CowString>>(custom_id: T) -> Self {
		let mut button = Self::new(ButtonStyle::Danger);
		button.component.custom_id = Some(custom_id.into());
		button
	}

	pub fn link<T: Into<CowString>>(url: T) -> Self {
		let mut button = Self::new(ButtonStyle::Link);
		button.component.url = Some(url.into());
		button
	}

	pub fn label<T: Into<CowString>>(mut self, label: T) -> Self {
		self.component.label = Some(label.into());
		self
	}

	pub fn disabled(mut self) -> Self {
		self.component.disabled = Some(true);
		self
	}

	pub fn emoji<T: Into<PartialEmoji>>(mut self, emoji: T) -> Self {
		self.component.emoji = Some(emoji.into());
		self
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
	interaction_id: InteractionId,
	token: &'a str,
	ir: request::InteractionResponse<'a>,
}

impl<'a> InteractionResponse<'a> {
	fn new(
		client: &'a Client,
		interaction_id: InteractionId,
		token: &'a str,
		component: bool,
	) -> Self {
		let response_type = if component {
			InteractionResponseType::UpdateMessage
		} else {
			InteractionResponseType::ChannelMessage
		};

		Self {
			client,
			interaction_id,
			token,
			ir: request::InteractionResponse {
				response_type,
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

	pub fn clear_embeds(mut self) -> Self {
		self.data().embeds = Some(Vec::new());
		self
	}

	pub fn embed(mut self, embed: Embed) -> Self {
		self.data()
			.embeds
			.get_or_insert_with(|| Vec::new())
			.push(embed);
		self
	}

	pub fn embeds<I>(mut self, embeds: I) -> Self
	where
		I: IntoIterator<Item = Embed>,
	{
		self.data().embeds = Some(embeds.into_iter().collect());
		self
	}

	pub fn clear_component_rows(mut self) -> Self {
		self.data().components = Some(Vec::new());
		self
	}

	pub fn component_row(mut self, row: RowComponent) -> Self {
		self.data()
			.components
			.get_or_insert_with(|| Vec::new())
			.push(row.component);
		self
	}

	pub fn component_rows<I>(mut self, rows: I) -> Self
	where
		I: IntoIterator<Item = RowComponent>,
	{
		self.data().components = Some(rows.into_iter().map(|r| r.component).collect());
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
			if data.content.is_none() && data.embeds.is_none() && data.components.is_none() {
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

pub struct EditInteractionResponse<'a> {
	client: &'a Client,
	application_id: ApplicationId,
	token: &'a str,
}
