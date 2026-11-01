extern crate proc_macro;
#[proc_macro]
pub fn generated(_: proc_macro::TokenStream) -> proc_macro::TokenStream {
    "pub fn proc_generated() { truth_dep::leaf(); }".parse().unwrap()
}
