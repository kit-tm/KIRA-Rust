pub trait Expiring {
    type Context;
    type Result;

    fn expire(&mut self, context: &Self::Context) -> Self::Result;
}
