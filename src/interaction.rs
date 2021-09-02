pub use crate::client::{ButtonComponent, RowComponent};
use crate::client::{Client, Error};
use crate::guild::Guild;
pub use discord_types::{AllowedMentions, Interaction};
use discord_types::{Embed, InteractionId};
use log::warn;
use std::borrow::Cow;
use std::future::Future;
use tokio::task::JoinHandle;

pub trait CanRespond {
	fn respond(&self, guild: &Guild) -> ResponseBuilder;
}

impl CanRespond for Interaction {
	fn respond(&self, guild: &Guild) -> ResponseBuilder {
		ResponseBuilder::new(self, guild.client())
	}
}

#[derive(Clone)]
pub struct ResponseBuilder {
	interaction_id: InteractionId,
	token: String,
	component: bool,
	client: Client,
	content: Option<Cow<'static, str>>,
	embeds: Option<Vec<Embed>>,
	rows: Option<Vec<RowComponent>>,
	ephemeral: bool,
	allowed_mentions: Option<AllowedMentions>,
}

impl ResponseBuilder {
	fn new(interaction: &Interaction, client: Client) -> Self {
		ResponseBuilder {
			interaction_id: interaction.id,
			token: interaction.token.clone(),
			component: interaction.is_component_interaction(),
			client,
			content: None,
			embeds: None,
			rows: None,
			ephemeral: false,
			allowed_mentions: None,
		}
	}

	pub fn interaction_id(&self) -> InteractionId {
		self.interaction_id
	}

	pub fn is_component_interaction(&self) -> bool {
		self.component
	}

	pub fn content<T: Into<Cow<'static, str>>>(mut self, content: T) -> Self {
		self.content = Some(content.into());
		self
	}

	pub fn clear_embeds(mut self) -> Self {
		self.embeds = Some(Vec::new());
		self
	}

	pub fn embed(mut self, embed: Embed) -> Self {
		self.embeds.get_or_insert_with(|| Vec::new()).push(embed);
		self
	}

	pub fn clear_component_rows(mut self) -> Self {
		self.rows = Some(Vec::new());
		self
	}

	pub fn component_row(mut self, row: RowComponent) -> Self {
		self.rows.get_or_insert_with(|| Vec::new()).push(row);
		self
	}

	pub fn ephemeral(mut self) -> Self {
		self.ephemeral = true;
		self
	}

	pub fn allowed_mentions(mut self, m: AllowedMentions) -> Self {
		self.allowed_mentions = Some(m);
		self
	}

	pub fn send(self) -> impl Future<Output = Result<(), Error>> {
		let Self {
			interaction_id,
			token,
			component,
			client,
			content,
			embeds,
			rows,
			ephemeral,
			allowed_mentions,
		} = self;

		async move {
			let mut res = client.interaction_response(interaction_id, &token, component);
			if let Some(content) = &content {
				res = res.content(content);
			}
			if let Some(embeds) = embeds {
				res = res.embeds(embeds);
			}
			if let Some(rows) = rows {
				res = res.component_rows(rows);
			}

			if ephemeral {
				res = res.ephemeral();
			}
			if let Some(m) = allowed_mentions {
				res = res.allowed_mentions(m);
			}
			res.send().await
		}
	}

	pub fn spawn(self) -> JoinHandle<Result<(), Error>> {
		let task = self.send();
		tokio::spawn(async move {
			let res = task.await;
			if let Err(e) = &res {
				warn!("Unable to send interaction response: {}", e);
			}
			res
		})
	}
}
