use crate::model::Device;
impl Device {
    pub fn inherent(&self) { /*@inherent_body*/truth_dep::leaf(); }
    pub fn associated() { /*@associated_body*/truth_dep::leaf(); }
}
impl truth_dep::Act for Device {
    fn act(&self) { /*@impl_body*/truth_dep::leaf(); }
}
