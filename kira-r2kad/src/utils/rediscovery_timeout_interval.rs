use std::collections::HashMap;
use std::time::Duration;

use rand::Rng;

/// Possible distances between two contacts.
#[derive(Debug, Eq, PartialEq, Hash)]
pub enum Distance {
    OverlayNeighbor,
    UnderlayNeighbor,
    Others,
}

/// A Map containing a duration for every possible [Distance].
#[derive(Debug)]
pub struct DistanceMap {
    values: HashMap<Distance, Duration>,
}

impl DistanceMap {
    pub fn new(overlay: Duration, underlay: Duration, others: Duration) -> Self {
        let mut map = HashMap::with_capacity(3);
        map.insert(Distance::OverlayNeighbor, overlay);
        map.insert(Distance::UnderlayNeighbor, underlay);
        map.insert(Distance::Others, others);
        Self { values: map }
    }
}

impl Default for DistanceMap {
    fn default() -> Self {
        Self::new(
            Duration::from_millis(500),
            Duration::from_secs(1),
            Duration::from_secs(2),
        )
    }
}

/// Interval generating random values in a given range based on distance and two parameters.
///
/// Used for rediscovery timeout calculation.
/// See [FailureHandling](crate::use_cases::failure_handling::FailureHandling) use case for additional information.
#[derive(Debug)]
pub struct RediscoveryTimeoutInterval {
    distance_map: DistanceMap,
    lower_interval_param: f64,
    upper_interval_param: f64,
}

impl Default for RediscoveryTimeoutInterval {
    fn default() -> Self {
        Self::new(0.5, 1.5, DistanceMap::default()).unwrap()
    }
}

impl RediscoveryTimeoutInterval {
    /// Creates a new interval.
    ///
    /// If the lower_param is greater than the upper interval [None] is returned.
    pub fn new(
        lower_interval_param: f64,
        upper_interval_param: f64,
        distance_map: DistanceMap,
    ) -> Option<Self> {
        if lower_interval_param > upper_interval_param {
            return None;
        }

        Some(Self {
            distance_map,
            lower_interval_param,
            upper_interval_param,
        })
    }

    /// Generates a new duration for the given distance.
    ///
    /// ## Calculation
    ///
    /// A random value in the interval `[lower_param * distance_base, upper_param * distance_base]`
    /// where `distance_base` is given by the [DistanceMap] used for this interval.
    pub fn next_duration(&self, distance: Distance) -> Duration {
        let distance_timeout = self.distance_map.values[&distance];
        let lower_bound =
            Duration::from_secs_f64(self.lower_interval_param * distance_timeout.as_secs_f64());
        let upper_bound =
            Duration::from_secs_f64(self.upper_interval_param * distance_timeout.as_secs_f64());
        if lower_bound == upper_bound {
            lower_bound
        } else {
            rand::rng().random_range(lower_bound..upper_bound)
        }
    }
}
