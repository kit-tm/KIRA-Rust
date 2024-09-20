use std::fmt::{Display, Formatter};


/// Represents network interface (as in 'hardware device') by interface index.
#[derive(Debug, Clone, Hash)]
pub struct NetworkInterface {
    pub index: u32,
    pub name: String
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

    pub fn is_tunnel_interface(&self) -> bool {
        self.name.starts_with("kira")
    }
}

#[cfg(test)]
impl NetworkInterface {
    // method only for tests
    // todo find a better way to make this work for tests
    pub fn with_name<S: Into<String>>(name: S) -> Self {
        Self {
            index: u32::MAX,
            name: name.into(),
        }
    }
}

impl PartialEq for NetworkInterface {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl Eq for NetworkInterface {}

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
