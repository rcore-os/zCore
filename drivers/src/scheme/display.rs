use super::Scheme;

pub trait DisplayScheme: Scheme {
    type DisplayInfo;

    fn info(&self) -> Self::DisplayInfo;
}
