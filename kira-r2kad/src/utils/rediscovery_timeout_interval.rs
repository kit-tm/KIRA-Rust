use std::{
    collections::HashMap,
    time::Duration,
};

use rand::RngExt as _;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_interval_rejects_inverted_params() {
        assert!(RediscoveryTimeoutInterval::new(1.5, 0.5, DistanceMap::default()).is_none());
        assert!(RediscoveryTimeoutInterval::new(0.5, 1.5, DistanceMap::default()).is_some());
    }

    #[test]
    fn default_interval_accepts_equal_params() {
        assert!(RediscoveryTimeoutInterval::new(1.0, 1.0, DistanceMap::default()).is_some());
    }

    #[test]
    fn default_distance_map_values() {
        let interval = RediscoveryTimeoutInterval::default();
        // base values: overlay 500ms, underlay 1s, others 2s; params 0.5..1.5
        let overlay = interval.next_duration(Distance::OverlayNeighbor);
        assert!(
            Duration::from_millis(250) <= overlay && overlay < Duration::from_millis(750),
            "overlay duration {overlay:?} outside [250ms, 750ms)"
        );
    }

    #[test]
    fn next_duration_respects_bounds_per_distance() {
        let interval = RediscoveryTimeoutInterval::new(
            0.5,
            1.5,
            DistanceMap::new(
                Duration::from_secs(10),
                Duration::from_secs(20),
                Duration::from_secs(40),
            ),
        )
        .unwrap();

        for _ in 0..100 {
            let overlay = interval.next_duration(Distance::OverlayNeighbor);
            assert!(
                Duration::from_secs(5) <= overlay && overlay < Duration::from_secs(15),
                "overlay duration {overlay:?} outside [5s, 15s)"
            );

            let underlay = interval.next_duration(Distance::UnderlayNeighbor);
            assert!(
                Duration::from_secs(10) <= underlay && underlay < Duration::from_secs(30),
                "underlay duration {underlay:?} outside [10s, 30s)"
            );

            let others = interval.next_duration(Distance::Others);
            assert!(
                Duration::from_secs(20) <= others && others < Duration::from_secs(60),
                "others duration {others:?} outside [20s, 60s)"
            );
        }
    }

    #[test]
    fn equal_params_return_exact_base() {
        let interval = RediscoveryTimeoutInterval::new(
            1.0,
            1.0,
            DistanceMap::new(
                Duration::from_secs(10),
                Duration::from_secs(20),
                Duration::from_secs(40),
            ),
        )
        .unwrap();

        assert_eq!(interval.next_duration(Distance::OverlayNeighbor), Duration::from_secs(10));
        assert_eq!(interval.next_duration(Distance::UnderlayNeighbor), Duration::from_secs(20));
        assert_eq!(interval.next_duration(Distance::Others), Duration::from_secs(40));
    }
}
