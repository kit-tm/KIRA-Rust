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
        let interface = interfaces.iter().find(|i| i.index == index).expect("Unable to find interface");
        Self { index, name: interface.name.clone() }
    }

    // todo use lazy initialization
    pub fn loopback() -> Self {
        let interfaces = pnet::datalink::interfaces();
        let interface = interfaces.iter().find(|i| i.is_loopback()).expect("Unable to find loopback interface");

        Self {
            index: interface.index,
            name: interface.name.clone(),
        }
    }
}

#[cfg(test)]
impl NetworkInterface {
    /// This method creates a dummy interface for test cases.
    ///
    /// This function does not check if the interface actually exists. Use with caution.<
    pub fn with_name<S: Into<String>>(name: S) -> Self {
        let interfaces = pnet::datalink::interfaces();
        let name = name.into();
        // try to find real index of interface if exists
        let index = interfaces.iter().find(|i| i.name == name)
            .map_or(u32::MAX, |i| i.index);

        Self { index, name }
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
