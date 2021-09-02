use crate::guild::Guild;
use discord_types::Message;
use std::future::Future;

pub trait CanEdit<'a> {
	fn edit(&'a self, guild: &'a Guild) -> EditBuilder<'a>;
}

impl<'a> CanEdit<'a> for Message {
	fn edit(&'a self, guild: &'a Guild) -> EditBuilder<'a> {
		EditBuilder::new(self, guild)
	}
}

pub struct EditBuilder<'a> {
	message: &'a Message,
	guild: &'a Guild,
}

impl<'a> EditBuilder<'a> {
	fn new(message: &'a Message, guild: &'a Guild) -> Self {
		EditBuilder { message, guild }
	}

	/*pub fn send(self) -> impl Future<Output = Result<(), Error>> {
		let Self { message, guild } = self;

		let client = guild.client();

		async move {
			// let mut res = client.edit
		}
	}*/
}
