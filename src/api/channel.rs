use super::Attachment;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CreateMessage {
	pub(crate) content: Option<String>,
	pub(crate) file: Option<Attachment>,
}

impl From<String> for CreateMessage {
	fn from(content: String) -> Self {
		Self {
			content: Some(content),
			file: None,
		}
	}
}

impl From<Attachment> for CreateMessage {
	fn from(file: Attachment) -> Self {
		Self {
			content: None,
			file: Some(file),
		}
	}
}

impl From<(String, Attachment)> for CreateMessage {
	fn from(content: (String, Attachment)) -> Self {
		Self {
			content: Some(content.0),
			file: Some(content.1),
		}
	}
}
