use super::Scheme;

pub struct DisplayInfo;

pub trait DisplayScheme: Scheme {
    fn info(&self) -> DisplayInfo;
}
