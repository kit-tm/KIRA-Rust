use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};

/// Represents network interface (as in 'hardware device') by interface index.
#[derive(Debug, Clone)]
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

    // todo use lazy initialization
    pub fn loopback() -> Self {
        let interfaces = pnet::datalink::interfaces();
        let interface = interfaces
            .iter()
            .find(|i| i.is_loopback())
            .expect("Unable to find loopback interface");

        Self {
            index: interface.index,
            name: interface.name.clone(),
        }
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

impl Hash for NetworkInterface {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
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
    use super::NetworkInterface;
    use pnet::datalink;
    use std::str::FromStr;

    impl<'a> From<&'a datalink::NetworkInterface> for NetworkInterface {
        fn from(iface: &'a datalink::NetworkInterface) -> Self {
            NetworkInterface::new(iface.index)
        }
    }

    impl FromStr for NetworkInterface {
        type Err = String;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            let iface_idx = s.parse::<u32>().ok();

            let interfaces = datalink::interfaces();

            let interface = interfaces
                .into_iter()
                .find(|iface| {
                    if let Some(idx) = iface_idx {
                        iface.index == idx
                    } else {
                        iface.name == s
                    }
                })
                .ok_or_else(|| {
                    format!("No interface found matching {s} by either name or index")
                })?;

            Ok(Self::from(&interface))
        }
    }
}
