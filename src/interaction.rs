use crate::client::Error;
use crate::guild::Guild;
use discord_types::Embed;
pub use discord_types::{AllowedMentions, Interaction};
use log::warn;
use std::borrow::Cow;
use std::future::Future;
use tokio::task::JoinHandle;

pub trait CanRespond<'a> {
	fn respond(&'a self, guild: &'a Guild) -> ResponseTask<'a>;
}

impl<'a> CanRespond<'a> for Interaction {
	fn respond(&'a self, guild: &'a Guild) -> ResponseTask<'a> {
		ResponseTask::new(self, guild)
	}
}

pub struct ResponseTask<'a> {
	interaction: &'a Interaction,
	guild: &'a Guild,
	content: Option<Cow<'static, str>>,
	embeds: Vec<Embed>,
	ephemeral: bool,
	allowed_mentions: Option<AllowedMentions>,
}

impl<'a> ResponseTask<'a> {
	fn new(interaction: &'a Interaction, guild: &'a Guild) -> Self {
		ResponseTask {
			interaction,
			guild,
			content: None,
			embeds: Vec::new(),
			ephemeral: false,
			allowed_mentions: None,
		}
	}

	pub fn content<T: Into<Cow<'static, str>>>(mut self, content: T) -> Self {
		self.content = Some(content.into());
		self
	}

	pub fn embed(mut self, embed: Embed) -> Self {
		self.embeds.push(embed);
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
			guild,
			interaction,
			content,
			embeds,
			ephemeral,
			allowed_mentions,
		} = self;

		let client = guild.client();
		let interaction_id = interaction.id;
		let token = interaction.token.clone();

		async move {
			let mut res = client.interaction_response(interaction_id, &token);
			if let Some(content) = &content {
				res = res.content(&content);
			}
			for embed in embeds {
				res = res.embed(embed);
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
