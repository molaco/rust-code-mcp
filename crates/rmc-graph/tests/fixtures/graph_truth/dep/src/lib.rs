#![no_std]
pub fn leaf() {}
pub fn register(_: fn()) {}
pub const COUNT: usize = 2;
pub struct Payload;
pub trait Act { fn act(&self); }
pub enum Event { Unit, Tuple(Payload), Record { value: Payload } }
