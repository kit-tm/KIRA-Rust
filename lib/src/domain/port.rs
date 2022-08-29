use std::fmt::{Display, Formatter};

#[cfg(feature = "pnet")]
pub use pnet_conversion::*;

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

#[cfg(feature = "pnet")]
mod pnet_conversion {
    use pnet::datalink::NetworkInterface;

    use crate::domain::Port;

    impl<'a> From<&'a NetworkInterface> for Port {
        fn from(iface: &'a NetworkInterface) -> Self {
            Port::new(iface.name.clone())
        }
    }
}
