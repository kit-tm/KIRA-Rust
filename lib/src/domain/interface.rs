use std::fmt::{Display, Formatter};

#[cfg(feature = "pnet")]
pub use pnet_conversion::*;

/// Represents network interface (as in 'hardware device') by name.
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct NetworkInterface {
    pub name: String,
}

impl NetworkInterface {
    pub fn new<S: Into<String>>(name: S) -> Self {
        Self { name: name.into() }
    }
}

impl Display for NetworkInterface {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "NetworkInterface [\"{}\"]", self.name)
    }
}

#[cfg(feature = "pnet")]
mod pnet_conversion {
    use pnet::datalink;

    use crate::domain::NetworkInterface;

    impl<'a> From<&'a datalink::NetworkInterface> for NetworkInterface {
        fn from(iface: &'a datalink::NetworkInterface) -> Self {
            NetworkInterface::new(iface.name.clone())
        }
    }
}
