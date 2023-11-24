use std::fmt::{Display, Formatter};

#[cfg(feature = "pnet")]
pub use pnet_conversion::*;

/// Represents network interface (as in 'hardware device') by interface index.
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct NetworkInterface {
    pub index: u32,
    pub name: String,
}

impl NetworkInterface {
    pub fn new(index: u32) -> Self {
        let interfaces = pnet::datalink::interfaces();
        let interface = interfaces
            .iter()
            .find(|i| i.index == index)
            .expect("Unable to find interface");
        Self {
            index,
            name: interface.name.clone(),
        }
    }

    pub fn dummy<S: Into<String>>(name: S) -> Self {
        Self {
            index: 0,
            name: name.into(),
        }
    }
}

impl Display for NetworkInterface {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "NetworkInterface index [\"{}\"]", self.index)
    }
}

#[cfg(feature = "pnet")]
mod pnet_conversion {
    use pnet::datalink;

    use crate::domain::NetworkInterface;

    impl<'a> From<&'a datalink::NetworkInterface> for NetworkInterface {
        fn from(iface: &'a datalink::NetworkInterface) -> Self {
            NetworkInterface::new(iface.index)
        }
    }
}
