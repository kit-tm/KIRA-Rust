use std::fmt::{Display, Formatter};

/// Represents a logical [Port] where a [Message] can be received from or sent to.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Port {
    All,
    Named(String),
}

impl Port {
    pub fn new(id: String) -> Self {
        Self::Named(id)
    }
}

impl Display for Port {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => write!(f, "All"),
            Self::Named(name) => write!(f, "Port [\"{}\"]", name),
        }
    }
}
