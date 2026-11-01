#![allow(dead_code)]
pub mod model;
mod remote;
use truth_dep::{/*@import_leaf*/leaf, leaf as renamed, register, Act};
use model::Device;
use truth_dep::COUNT as renamed_count;
pub fn format_capture() { let _ = /*@format_alias*/format_args!("{renamed_count}"); }
pub fn references_only() { register(/*@reference_only*/leaf); }

pub fn direct() {
    /*@direct*/leaf();
    /*@renamed*/renamed();
    truth_dep::/*@qualified*/leaf();
    /*@register*/register(/*@fn_value*/leaf);
    let f = /*@local_fn_value*/leaf;
    /*@indirect*/f();
}
pub fn methods(d: &Device) {
    d./*@inherent_call*/inherent();
    Device::/*@associated_call*/associated();
    d./*@concrete_trait*/act();
    <Device as Act>::/*@ufcs_trait*/act(d);
}
pub fn generic<T: Act>(d: &T) { d./*@generic_call*/act(); }
pub fn dynamic(d: &dyn Act) { d./*@dyn_call*/act(); }
pub fn closures() {
    let c = || { /*@closure_call*/leaf(); };
    c();
    fn inner() { truth_dep::/*@nested_body*/leaf(); }
    /*@nested_call*/inner();
}
pub mod inline {
    pub fn leaf() {}
    pub fn run() { super::/*@inline_outer*/leaf(); /*@inline_local*/leaf(); }
}
pub fn data(d: &mut Device) {
    let _: Option</*@body_type*/model::Alias> = None;
    let _ = &d./*@field_read*/field;
    d./*@field_write*/field = truth_dep::/*@constructor*/Payload;
    let _ = model::/*@const_read*/SIZE;
    let _ = truth_dep::Event::/*@variant_unit*/Unit;
    let _ = truth_dep::Event::/*@variant_tuple*/Tuple(truth_dep::Payload);
    let _ = truth_dep::Event::/*@variant_record*/Record { value: truth_dep::Payload };
}
macro_rules! twice { ($f:path) => { $f(); $f(); } }
pub fn macros() { twice!(truth_dep::/*@macro_twice*/leaf); }
macro_rules! define_fn { () => { pub fn macro_generated() { truth_dep::/*@macro_body*/leaf(); } } }
define_fn!();
pub fn use_generated() { /*@macro_generated_call*/macro_generated(); }
truth_macros::generated!();
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
pub fn use_proc_generated() { /*@proc_generated_call*/proc_generated(); }
pub fn use_build_generated() { /*@build_generated_call*/build_generated(); }
#[cfg(feature = "extra")]
pub fn enabled() { /*@feature_on*/leaf(); }
#[cfg(not(feature = "extra"))]
pub fn disabled() { /*@feature_off*/leaf(); }
#[cfg(test)]
mod tests { #[test] fn unit() { truth_dep::/*@unit_test*/leaf(); } }
#[test]
fn bare_test() { /*@bare_test*/leaf(); }
pub fn helper() { truth_dep::/*@lib_helper*/leaf(); }
pub fn entry() { /*@lib_entry*/helper(); }
