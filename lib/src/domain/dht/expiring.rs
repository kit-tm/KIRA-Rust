use super::strategies::timeout_strategy::TimeoutStrategy;

/// This trait provides the functionality to expire itself.
/// 
/// This trait can be used together with the [TimeoutStrategy]
/// to determine dynamically if a value does expire.
/// 
/// ## Difference to [TimeoutStrategy]
/// 
/// * [TimeoutStrategy]: Provides the decision-making if a value is actually expired.
/// * [Expiring]: Provides the functionality to expire a value.
pub trait Expiring {
    /// Context used to expire itself
    type Context;
    
    /// Result of the [Self::expire] call
    type Result;

    /// This function provides the functionality to determine what
    /// should happen on expiring itself
    ///
    /// This function should always invoke the expire process
    /// independently of the fact that the value may not be expired.
    /// It is the responsibility of the caller to determine if a value is expired. 
    fn expire(&mut self, context: &Self::Context) -> Self::Result;
}
