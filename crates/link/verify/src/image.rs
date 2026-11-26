//! (to come)
pub struct Program<'a> {
    pub image: &'a [u8],
    pub pdb: &'a [u8],
}
pub fn compare(_a: Program<'_>, _b: Program<'_>) -> Result<crate::report::Report, String> {
    Err("not yet".into())
}
