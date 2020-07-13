pub use channel::*;
use serde::Serialize;

pub mod channel;

#[derive(Debug, Serialize)]
pub struct Attachment {
	pub(crate) name: String,
	pub(crate) data: Vec<u8>,
}

impl Attachment {
	pub fn new(name: String, data: Vec<u8>) -> Self {
		Self { name, data }
	}
}
