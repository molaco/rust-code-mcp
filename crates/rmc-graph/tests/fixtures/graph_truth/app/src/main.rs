fn helper() { truth_dep::/*@bin_helper*/register(truth_dep::leaf); }
fn main() { /*@bin_entry*/helper(); truth_app::entry(); }
