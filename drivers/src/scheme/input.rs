use super::Scheme;

pub struct InputState;

pub trait InputScheme: Scheme {
    fn state(&self) -> InputState;
}
